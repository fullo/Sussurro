//! macOS 14.2+ (#140): a **Core Audio process tap** on every process but
//! Sussurro (so its own beeps stay out), read through a private aggregate
//! device whose clock is the default output — Apple's "capturing system
//! audio with Core Audio taps" recipe, with no virtual device.
//!
//! The app's minimum stays macOS 11.0: `CATapDescription` and
//! `AudioHardwareCreateProcessTap` (not in `objc2-core-audio` 0.3) are
//! looked up at run time (`AnyClass::get`, `dlsym`), never linked, and
//! [`check`] refuses below 14.2. The first capture makes macOS ask for the
//! "System Audio Recording" permission (`NSAudioCaptureUsageDescription` in
//! `Info.plist`); if it is denied the tap delivers silence.

use super::{version, Unavailable};
use crate::audio::recorder::TARGET_RATE;
use crate::audio::resample::StreamResampler;
use crate::sources::system::Capture;
use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyNominalSampleRate, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyTranslatePIDToProcessObject, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioObjectUnknown,
    kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey,
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareDestroyAggregateDevice,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
};
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{
    NSArray, NSDictionary, NSNumber, NSObject, NSProcessInfo, NSString, NSUUID,
};
use std::ffi::{c_void, CStr};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

type OSStatus = i32;
type CreateProcessTap = unsafe extern "C" fn(*mut AnyObject, *mut AudioObjectID) -> OSStatus;
type DestroyProcessTap = unsafe extern "C" fn(AudioObjectID) -> OSStatus;

/// `kAudioTapPropertyFormat` ('tfmt', AudioHardwareTapping.h, 14.2+).
const TAP_PROPERTY_FORMAT: u32 = u32::from_be_bytes(*b"tfmt");
/// `CATapUnmuted`: the others keep hearing the call.
const TAP_UNMUTED: isize = 0;

/// The running macOS version, from `NSProcessInfo`.
pub fn os_version() -> version::OsVersion {
    let v = NSProcessInfo::processInfo().operatingSystemVersion();
    (
        v.majorVersion as u64,
        v.minorVersion as u64,
        v.patchVersion as u64,
    )
}

/// The tap API, looked up at run time (absent before 14.2).
struct TapApi {
    class: &'static AnyClass,
    create: CreateProcessTap,
    destroy: DestroyProcessTap,
}

fn tap_api() -> Option<TapApi> {
    let class = AnyClass::get(c"CATapDescription")?;
    // SAFETY: dlsym with RTLD_DEFAULT and NUL-terminated names; CoreAudio
    // is already loaded (cpal links it). The signatures are the ones in
    // AudioHardwareTapping.h.
    unsafe {
        let create = libc::dlsym(
            libc::RTLD_DEFAULT,
            c"AudioHardwareCreateProcessTap".as_ptr(),
        );
        let destroy = libc::dlsym(
            libc::RTLD_DEFAULT,
            c"AudioHardwareDestroyProcessTap".as_ptr(),
        );
        if create.is_null() || destroy.is_null() {
            return None;
        }
        Some(TapApi {
            class,
            create: std::mem::transmute::<*mut c_void, CreateProcessTap>(create),
            destroy: std::mem::transmute::<*mut c_void, DestroyProcessTap>(destroy),
        })
    }
}

/// Whether the native choice can be offered; `Ok` carries the output
/// device's name (what the tap is clocked by), when known.
pub fn check() -> Result<Option<String>, Unavailable> {
    let v = os_version();
    if !version::supports_process_tap(v) {
        return Err(Unavailable::MacOsTooOld {
            version: version::display(v),
        });
    }
    if tap_api().is_none() {
        return Err(Unavailable::TapApiMissing);
    }
    if default_output_device().is_none() {
        return Err(Unavailable::NoOutputDevice);
    }
    Ok(crate::audio::recorder::default_output_device_name())
}

fn address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Read a fixed-size property. `qualifier`: optional qualifier bytes.
///
/// # Safety
/// `T` must be the property's type (plain data, valid when zeroed).
unsafe fn get_property<T: Copy>(
    object: AudioObjectID,
    selector: u32,
    qualifier: Option<&[u8]>,
) -> Option<T> {
    let addr = address(selector);
    let mut value: T = std::mem::zeroed();
    let mut size = std::mem::size_of::<T>() as u32;
    let (q_size, q_ptr) = match qualifier {
        Some(q) => (q.len() as u32, q.as_ptr() as *const c_void),
        None => (0, std::ptr::null()),
    };
    let status = AudioObjectGetPropertyData(
        object,
        NonNull::from(&addr),
        q_size,
        q_ptr,
        NonNull::from(&mut size),
        NonNull::from(&mut value).cast(),
    );
    (status == 0).then_some(value)
}

fn default_output_device() -> Option<AudioObjectID> {
    // SAFETY: the property is an AudioObjectID.
    let id: AudioObjectID = unsafe {
        get_property(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyDefaultOutputDevice,
            None,
        )
    }?;
    (id != kAudioObjectUnknown).then_some(id)
}

/// A device's UID (a +1 CFString, bridged to NSString).
fn device_uid(device: AudioObjectID) -> Option<Retained<NSString>> {
    // SAFETY: the property is a CFStringRef the caller owns; NSString is
    // toll-free bridged to CFString.
    unsafe {
        let ptr: usize = get_property(device, kAudioDevicePropertyDeviceUID, None)?;
        Retained::from_raw(ptr as *mut NSString)
    }
}

/// Sussurro's own Core Audio process object, excluded from the tap.
fn own_process_object() -> Option<AudioObjectID> {
    let pid = (std::process::id() as i32).to_ne_bytes();
    // SAFETY: the qualifier is a pid_t, the property an AudioObjectID.
    let id: AudioObjectID = unsafe {
        get_property(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyTranslatePIDToProcessObject,
            Some(&pid),
        )
    }?;
    (id != kAudioObjectUnknown).then_some(id)
}

/// Any Foundation object as `id` (for heterogeneous dictionaries).
fn any(o: &NSObject) -> &AnyObject {
    o
}

fn key(k: &CStr) -> Retained<NSString> {
    NSString::from_str(k.to_str().unwrap_or_default())
}

/// What the IO proc writes into (owned by [`TapCapture`], freed after the
/// IO proc is destroyed).
struct IoState {
    resampler: Mutex<StreamResampler>,
    buf: Arc<Mutex<Vec<f32>>>,
    /// IO cycles seen (diagnostics).
    cycles: AtomicU64,
}

/// The aggregate device's IO proc: mixes every input channel to mono and
/// converts it to 16 kHz. The tap's samples are 32-bit float.
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> OSStatus {
    // SAFETY: `client` is the IoState registered with this proc; it lives
    // until after AudioDeviceDestroyIOProcID. The buffer list holds
    // `mNumberBuffers` buffers of f32 samples.
    let state = &*(client as *const IoState);
    state.cycles.fetch_add(1, Ordering::Relaxed);
    let list = input.as_ref();
    let buffers = std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize);
    let mono = mix_to_mono(buffers.iter().filter(|b| !b.mData.is_null()).map(|b| {
        let channels = b.mNumberChannels.max(1) as usize;
        let len = b.mDataByteSize as usize / std::mem::size_of::<f32>();
        (
            std::slice::from_raw_parts(b.mData as *const f32, len),
            channels,
        )
    }));
    if !mono.is_empty() {
        let out = state.resampler.lock().unwrap().push(&mono);
        state.buf.lock().unwrap().extend_from_slice(&out);
    }
    0
}

/// Every channel of every buffer (each interleaved with its own channel
/// count) averaged into one mono signal, as long as the shortest buffer.
fn mix_to_mono<'a>(buffers: impl Iterator<Item = (&'a [f32], usize)>) -> Vec<f32> {
    let buffers: Vec<(&[f32], usize)> = buffers.collect();
    let frames = buffers
        .iter()
        .map(|(d, ch)| d.len() / ch)
        .min()
        .unwrap_or(0);
    let total: usize = buffers.iter().map(|(_, ch)| ch).sum();
    if frames == 0 || total == 0 {
        return Vec::new();
    }
    let scale = 1.0 / total as f32;
    (0..frames)
        .map(|i| {
            buffers
                .iter()
                .map(|(d, ch)| d[i * ch..(i + 1) * ch].iter().sum::<f32>())
                .sum::<f32>()
                * scale
        })
        .collect()
}

/// A running tap + aggregate device + IO proc.
pub struct TapCapture {
    api_destroy: DestroyProcessTap,
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    state: *mut IoState,
    buf: Arc<Mutex<Vec<f32>>>,
    running: bool,
    /// Sample rate the IO proc delivers (diagnostics).
    pub rate: f64,
    silence: SilenceWatch,
    hint: Option<String>,
}

// SAFETY: the raw pointers are only used by `stop`, which tears the IO
// proc down before freeing `state`; Core Audio object ids are plain ints.
unsafe impl Send for TapCapture {}

impl TapCapture {
    pub fn start() -> anyhow::Result<Self> {
        check().map_err(|why| anyhow::anyhow!(why.message()))?;
        let api = tap_api().ok_or_else(|| anyhow::anyhow!(Unavailable::TapApiMissing.message()))?;
        let output = default_output_device()
            .ok_or_else(|| anyhow::anyhow!(Unavailable::NoOutputDevice.message()))?;
        let output_uid = device_uid(output)
            .ok_or_else(|| anyhow::anyhow!("could not read the output device's UID"))?;

        // The tap: every process but ours, stereo, private, unmuted.
        let excluded: Vec<Retained<NSNumber>> = own_process_object()
            .map(NSNumber::numberWithUnsignedInt)
            .into_iter()
            .collect();
        let excluded = NSArray::from_retained_slice(&excluded);
        // SAFETY: CATapDescription's documented initializer and setters.
        let desc: Retained<AnyObject> = unsafe {
            let alloc: Allocated<AnyObject> = msg_send![api.class, alloc];
            let desc: Option<Retained<AnyObject>> =
                msg_send![alloc, initStereoGlobalTapButExcludeProcesses: &*excluded];
            let desc = desc.ok_or_else(|| anyhow::anyhow!("could not describe the audio tap"))?;
            let _: () = msg_send![&*desc, setPrivate: true];
            let _: () = msg_send![&*desc, setMuteBehavior: TAP_UNMUTED];
            let _: () = msg_send![&*desc, setName: &*NSString::from_str("Sussurro system audio")];
            desc
        };
        // SAFETY: `UUID` returns the description's NSUUID.
        let tap_uuid: Retained<NSUUID> = unsafe { msg_send![&*desc, UUID] };
        let tap_uuid = tap_uuid.UUIDString();

        let mut tap: AudioObjectID = kAudioObjectUnknown;
        // SAFETY: a valid CATapDescription and out pointer.
        let status = unsafe { (api.create)(Retained::as_ptr(&desc) as *mut AnyObject, &mut tap) };
        if status != 0 || tap == kAudioObjectUnknown {
            anyhow::bail!("macOS refused the system audio tap (error {status})");
        }

        let mut me = Self {
            api_destroy: api.destroy,
            tap,
            aggregate: kAudioObjectUnknown,
            proc_id: None,
            state: std::ptr::null_mut(),
            buf: Arc::default(),
            running: false,
            rate: 0.0,
            silence: SilenceWatch::default(),
            hint: None,
        };
        // From here on, `me` tears down whatever was built if a step fails.
        me.open_aggregate(&output_uid, &tap_uuid)?;
        me.rate = unsafe {
            get_property::<f64>(me.aggregate, kAudioDevicePropertyNominalSampleRate, None)
        }
        .filter(|r| *r > 0.0)
        .or_else(|| {
            // SAFETY: the tap's format is an AudioStreamBasicDescription.
            unsafe { get_property::<AudioStreamBasicDescription>(tap, TAP_PROPERTY_FORMAT, None) }
                .map(|f| f.mSampleRate)
                .filter(|r| *r > 0.0)
        })
        .ok_or_else(|| anyhow::anyhow!("could not read the system audio sample rate"))?;

        let state = Box::into_raw(Box::new(IoState {
            resampler: Mutex::new(StreamResampler::new(1, me.rate.round() as u32, TARGET_RATE)),
            buf: me.buf.clone(),
            cycles: AtomicU64::new(0),
        }));
        me.state = state;
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: `io_proc` matches AudioDeviceIOProc; `state` outlives it.
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                me.aggregate,
                Some(io_proc),
                state as *mut c_void,
                NonNull::from(&mut proc_id),
            )
        };
        if status != 0 || proc_id.is_none() {
            anyhow::bail!("could not read the system audio tap (error {status})");
        }
        me.proc_id = proc_id;
        // SAFETY: a device and IO proc created above.
        let status = unsafe { AudioDeviceStart(me.aggregate, me.proc_id) };
        if status != 0 {
            anyhow::bail!("could not start the system audio tap (error {status})");
        }
        me.running = true;
        Ok(me)
    }

    /// A private aggregate device clocked by the output device, carrying
    /// the tap as its input.
    fn open_aggregate(&mut self, output_uid: &NSString, tap_uuid: &NSString) -> anyhow::Result<()> {
        let yes = NSNumber::numberWithBool(true);
        let no = NSNumber::numberWithBool(false);
        let uid = NSString::from_str(&format!("com.sussurro.app.system-audio.{tap_uuid}"));
        let name = NSString::from_str("Sussurro system audio");
        let sub_device: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices(&[&*key(kAudioSubDeviceUIDKey)], &[any(output_uid)]);
        let sub_tap: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
            &[
                &*key(kAudioSubTapUIDKey),
                &*key(kAudioSubTapDriftCompensationKey),
            ],
            &[any(tap_uuid), any(&yes)],
        );
        let sub_devices = NSArray::from_retained_slice(&[sub_device]);
        let taps = NSArray::from_retained_slice(&[sub_tap]);
        let keys = [
            key(kAudioAggregateDeviceUIDKey),
            key(kAudioAggregateDeviceNameKey),
            key(kAudioAggregateDeviceIsPrivateKey),
            key(kAudioAggregateDeviceIsStackedKey),
            key(kAudioAggregateDeviceTapAutoStartKey),
            key(kAudioAggregateDeviceMainSubDeviceKey),
            key(kAudioAggregateDeviceSubDeviceListKey),
            key(kAudioAggregateDeviceTapListKey),
        ];
        let values: [&AnyObject; 8] = [
            any(&uid),
            any(&name),
            any(&yes),
            any(&no),
            any(&yes),
            any(output_uid),
            any(&sub_devices),
            any(&taps),
        ];
        let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let description: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices(&key_refs, &values);
        let mut aggregate: AudioObjectID = kAudioObjectUnknown;
        // SAFETY: NSDictionary is toll-free bridged to CFDictionary.
        let status = unsafe {
            let cf = &*(Retained::as_ptr(&description) as *const CFDictionary);
            AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate))
        };
        if status != 0 || aggregate == kAudioObjectUnknown {
            anyhow::bail!("could not create the system audio device (error {status})");
        }
        self.aggregate = aggregate;
        Ok(())
    }

    /// IO cycles delivered so far (diagnostics, tests).
    pub fn cycles(&self) -> u64 {
        if self.state.is_null() {
            return 0;
        }
        // SAFETY: `state` is live until `teardown`.
        unsafe { (*self.state).cycles.load(Ordering::Relaxed) }
    }

    fn teardown(&mut self) -> Vec<f32> {
        // SAFETY: each object is destroyed once, IO proc first, and
        // `state` is freed only after its IO proc is gone.
        unsafe {
            if self.running {
                AudioDeviceStop(self.aggregate, self.proc_id);
                self.running = false;
            }
            if self.proc_id.is_some() {
                AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
                self.proc_id = None;
            }
            if self.aggregate != kAudioObjectUnknown {
                AudioHardwareDestroyAggregateDevice(self.aggregate);
                self.aggregate = kAudioObjectUnknown;
            }
            if self.tap != kAudioObjectUnknown {
                (self.api_destroy)(self.tap);
                self.tap = kAudioObjectUnknown;
            }
            let mut tail = std::mem::take(&mut *self.buf.lock().unwrap());
            if !self.state.is_null() {
                let state = Box::from_raw(self.state);
                self.state = std::ptr::null_mut();
                tail.extend(state.resampler.lock().unwrap().flush());
            }
            tail
        }
    }
}

/// A tap whose permission was denied (or never asked, as for a process
/// without `NSAudioCaptureUsageDescription`) delivers digital silence, not
/// an error. After [`SILENCE_HINT_MS`] of exact zeros the user gets one
/// hint — it may also just be a quiet start, so it is worded as a check.
pub const SILENCE_HINT_MS: u64 = 20_000;

/// Watches for a capture of nothing but exact zeros. Pure.
#[derive(Default)]
pub struct SilenceWatch {
    zeros: u64,
    heard: bool,
    hinted: bool,
}

impl SilenceWatch {
    /// Feed 16 kHz samples; `Some` once, when only zeros have come for
    /// [`SILENCE_HINT_MS`].
    pub fn feed(&mut self, samples: &[f32]) -> Option<String> {
        if self.heard || self.hinted {
            return None;
        }
        if samples.iter().any(|&x| x != 0.0) {
            self.heard = true;
            return None;
        }
        self.zeros += samples.len() as u64;
        if self.zeros < SILENCE_HINT_MS * TARGET_RATE as u64 / 1000 {
            return None;
        }
        self.hinted = true;
        Some(format!(
            "No sound from this computer in the first {} s. If the call is already playing, allow Sussurro in System Settings → Privacy & Security → Screen & System Audio Recording (System Audio Recording Only), then start a new recording.",
            SILENCE_HINT_MS / 1000
        ))
    }
}

impl Capture for TapCapture {
    fn take(&mut self) -> Vec<f32> {
        let out = std::mem::take(&mut *self.buf.lock().unwrap());
        if let Some(hint) = self.silence.feed(&out) {
            self.hint = Some(hint);
        }
        out
    }
    fn take_hint(&mut self) -> Option<String> {
        self.hint.take()
    }
    fn failed(&self) -> bool {
        // A device that goes away stops the IO cycles: the session's stall
        // check ends the channel (the tap delivers zeros, not nothing,
        // while nothing plays).
        false
    }
    fn stop(&mut self) -> Vec<f32> {
        self.teardown()
    }
}

impl Drop for TapCapture {
    fn drop(&mut self) {
        self.teardown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_of_every_buffer_are_averaged() {
        // One interleaved stereo buffer.
        let stereo = [1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        assert_eq!(
            mix_to_mono([(&stereo[..], 2)].into_iter()),
            vec![0.5, 0.5, 0.0]
        );
        // Two non-interleaved mono buffers (the shorter one bounds it).
        let l = [1.0, 1.0, 1.0];
        let r = [0.0, -1.0];
        assert_eq!(
            mix_to_mono([(&l[..], 1), (&r[..], 1)].into_iter()),
            vec![0.5, 0.0]
        );
        assert!(mix_to_mono(std::iter::empty()).is_empty());
    }

    #[test]
    fn only_digital_silence_gets_one_permission_hint() {
        let second = vec![0.0f32; TARGET_RATE as usize];
        let mut w = SilenceWatch::default();
        for _ in 0..SILENCE_HINT_MS / 1000 - 1 {
            assert!(w.feed(&second).is_none());
        }
        let hint = w.feed(&second).expect("hint after 20 s of zeros");
        assert!(hint.contains("System Audio Recording"), "{hint}");
        assert!(w.feed(&second).is_none(), "only once");

        // Any real sample (even very quiet) means the tap works.
        let mut w = SilenceWatch::default();
        let mut quiet = second.clone();
        quiet[100] = 1e-6;
        assert!(w.feed(&quiet).is_none());
        for _ in 0..60 {
            assert!(w.feed(&second).is_none());
        }
    }

    #[test]
    fn this_mac_reports_its_version() {
        let v = os_version();
        assert!(v.0 >= 11, "{v:?}");
        // The probe agrees with the version gate.
        let probe = super::super::probe();
        if !version::supports_process_tap(v) {
            assert!(!probe.available);
        }
    }

    /// Opens a real tap (asks for the System Audio Recording permission).
    /// Play something first, e.g. `afplay /System/Library/Sounds/Submarine.aiff`
    /// in a loop. Run manually:
    /// `cargo test tap_delivers_audio -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn tap_delivers_audio() {
        let mut c = TapCapture::start().unwrap();
        println!("tap rate {} Hz", c.rate);
        let mut got = Vec::new();
        for _ in 0..12 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            got.extend(c.take());
        }
        let cycles = c.cycles();
        got.extend(c.stop());
        let peak = got.iter().fold(0f32, |m, x| m.max(x.abs()));
        println!(
            "{} samples at 16 kHz in 3 s, {cycles} IO cycles, peak {peak:.4}",
            got.len()
        );
        assert!(
            got.len() > 40_000,
            "the tap delivered {} samples",
            got.len()
        );
    }
}
