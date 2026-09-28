//! Signed metadata (C2PA) on generated speech (#257 part 2, E17 item 2;
//! Code of Practice 1.1.1 and 1.3): the third marking layer, after the tags
//! and the AudioSeal watermark of [`super::marking`].
//!
//! **Who signs**: each install has its own **self-signed** certificate
//! chain — a root made once and thrown away after it signs one end-entity
//! certificate (ECDSA P-256, `digitalSignature`, EKU `emailProtection`,
//! valid [`VALIDITY_YEARS`] years). Nothing names the user: the subject is
//! `CN=Sussurro install <8 hex of the public key>`,
//! `O=`[`SUBJECT_ORG`]. It is made **on first need** — the first time the
//! read-aloud module generates a file — never at install or startup.
//! Verifiers show such a manifest as valid from an unknown signer: no
//! trust-list signing (the maintainer's decision, #257). Files signed by the
//! same install can be linked to each other through the certificate; the
//! README says so.
//!
//! **Where it lives**: the end-entity private key (PKCS#8 PEM) only in the
//! OS credential store ([`crate::secrets`], service `com.sussurro.app`,
//! account [`KEY_ACCOUNT`]), written and read back before it counts; the
//! public certificate chain in `<app data>/`[`SIGNING_DIR`]`/`[`CHAIN_FILE`]
//! (folder 0700, file 0600 on Unix). **No clear-text fallback**: without a
//! working credential store nothing is signed — the file is still
//! generated, with the watermark and the tags, and the frontmatter records
//! why in `synthetic.<file>.unsigned`. Never in settings.json, exports,
//! diagnostics or logs; [`Identity`]'s `Debug` prints the name only.
//!
//! **What is signed** ([`manifest_definition`], no privacy-sensitive data,
//! Code 1.3): claim generator `Sussurro <version>`, one `c2pa.created`
//! action with the IPTC digital source type `trainedAlgorithmicMedia`, the
//! time (UTC, this computer's clock — no time-stamp authority, nothing
//! goes on the network), and the engine, voice and language. A fixed title,
//! never the document's name. No soft-binding assertion (the AudioSeal entry
//! of the C2PA list belongs to a third party, E17).
//!
//! **Where it goes**: Ogg can't carry a C2PA manifest, so a saved
//! `speech*.opus` gets a **sidecar** `speech*.c2pa` ([`sign_sidecar`], a
//! data hash over the whole file), and so does a *Listen* file. A WAV
//! (the Models → Voices preview) gets the manifest **embedded**
//! ([`embed_in_wav`]). The app has no other export of speech files.
//!
//! **Verification** ([`verify_sidecar`], [`verify_embedded`]) is local:
//! c2pa-rs without an HTTP client, no trust anchors, no OCSP fetch.

use super::marking::{Provenance, DIGITAL_SOURCE_TYPE, GENERATOR};
use crate::secrets::SecretStore;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Credential-store account of the signing key.
pub const KEY_ACCOUNT: &str = "c2pa-signing-key";
/// Folder of the certificate chain, under the app data folder.
pub const SIGNING_DIR: &str = "c2pa";
/// The public certificate chain (end entity, then root), PEM.
pub const CHAIN_FILE: &str = "signing-chain.pem";
/// Extension of a sidecar manifest.
pub const SIDECAR_EXT: &str = "c2pa";
/// Largest sidecar read by *Check a file* (ours are ~3.4 KB).
pub const MAX_SIDECAR_BYTES: u64 = 1024 * 1024;
/// Organisation of the certificates: says what they are.
pub const SUBJECT_ORG: &str = "Sussurro (self-signed, one per install)";
/// Lifetime of the certificate. With no time-stamp authority a verifier
/// checks the certificate against *its* clock, so it must outlive the files.
pub const VALIDITY_YEARS: i32 = 30;
/// The manifest's title: fixed, never a document's name.
pub const TITLE: &str = "Synthetic speech";
/// Format the sidecar hash is made against (Ogg Opus).
const OGG_FORMAT: &str = "audio/ogg";

// ---- the identity ---------------------------------------------------------------

/// This install's signer (the key from the credential store, the chain from
/// app data). Holds no PEM text.
pub struct Identity {
    signer: c2pa::BoxedSigner,
    /// The certificate's common name (`Sussurro install 1a2b3c4d`).
    pub name: String,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Where the chain file of `dir` is.
pub fn chain_path(dir: &Path) -> PathBuf {
    dir.join(CHAIN_FILE)
}

/// A fresh chain: `(chain PEM, end-entity key PEM, common name)`.
fn generate() -> Result<(String, String, String)> {
    use chrono::Datelike;
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
        IsCa, Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
    };
    let today = chrono::Utc::now().date_naive() - chrono::Duration::days(1);
    let from = rcgen::date_time_ymd(today.year(), today.month() as u8, today.day() as u8);
    // 28th: every month has one.
    let until = rcgen::date_time_ymd(
        today.year() + VALIDITY_YEARS,
        today.month() as u8,
        today.day().min(28) as u8,
    );
    let ee_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let name = name_for(&ee_key);
    let dn = |cn: &str| {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, cn);
        dn.push(DnType::OrganizationName, SUBJECT_ORG);
        dn
    };

    let root_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let mut root = CertificateParams::default();
    root.distinguished_name = dn(&format!("{name} root"));
    root.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    root.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    root.not_before = from;
    root.not_after = until;
    let root_cert = root.self_signed(&root_key)?;
    // The root key signs this one certificate and is dropped: nothing
    // else can ever be issued under this root.
    let issuer = Issuer::new(root, root_key);

    let mut ee = CertificateParams::default();
    ee.distinguished_name = dn(&name);
    ee.is_ca = IsCa::ExplicitNoCa;
    ee.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    ee.extended_key_usages = vec![ExtendedKeyUsagePurpose::EmailProtection];
    ee.use_authority_key_identifier_extension = true;
    ee.not_before = from;
    ee.not_after = until;
    let ee_cert = ee.signed_by(&ee_key, &issuer)?;
    Ok((
        format!("{}{}", ee_cert.pem(), root_cert.pem()),
        ee_key.serialize_pem(),
        name,
    ))
}

/// The certificate name of a key: `Sussurro install <8 hex of the SHA-256
/// of its public key>` — stable per key, says nothing about the user.
fn name_for(key: &rcgen::KeyPair) -> String {
    let id = &crate::archive::store::sha256_hex(key.public_key_raw())[..8];
    format!("{GENERATOR} install {id}")
}

/// The name of the stored key (PKCS#8 PEM).
fn name_of(key_pem: &str) -> Option<String> {
    rcgen::KeyPair::from_pem(key_pem).ok().map(|k| name_for(&k))
}

fn signer(chain_pem: &str, key_pem: &str) -> Result<c2pa::BoxedSigner> {
    c2pa::create_signer::from_keys(
        chain_pem.as_bytes(),
        key_pem.as_bytes(),
        c2pa::SigningAlg::Es256,
        None,
    )
    .map_err(|e| anyhow!("the signing certificate can't be used: {e}"))
}

/// The identity signs a tiny file and the signature verifies: the key
/// matches the chain and c2pa accepts the certificate (still valid now).
fn self_test(identity: &Identity) -> Result<()> {
    let probe = b"OggS sussurro signing self-test";
    let p = Provenance {
        engine: "self-test".into(),
        voice: "self-test".into(),
        language: "en".into(),
    };
    let manifest = sign_stream(
        identity,
        &p,
        "2026-01-01T00:00:00Z",
        &mut Cursor::new(probe),
    )?;
    let layer = verify_sidecar(&manifest, &mut Cursor::new(probe));
    if layer.status != SignatureStatus::Valid {
        bail!("self-test signature does not verify: {}", layer.problem);
    }
    Ok(())
}

fn write_chain(dir: &Path, chain_pem: &str) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    crate::settings::write_private_atomic(&chain_path(dir), chain_pem.as_bytes())
        .with_context(|| format!("saving {}", chain_path(dir).display()))
}

fn read_chain(dir: &Path) -> Option<String> {
    let path = chain_path(dir);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.file_type().is_file() || meta.len() > 64 * 1024 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// This install's signing identity: the stored one, or a new one made and
/// stored now (the key in `store`, verified by reading it back; the chain
/// in `dir`). `Err` = nothing can be signed, with the reason for the user
/// (never a secret): no working credential store is the usual one.
pub fn load_or_create(dir: &Path, store: &dyn SecretStore) -> Result<Identity, String> {
    let stored = store.get(KEY_ACCOUNT).map_err(|e| {
        format!(
            "{} is not available ({e}), and the signing key is kept nowhere else",
            crate::secrets::store_name()
        )
    })?;
    if let (Some(key), Some(chain)) = (stored.as_deref(), read_chain(dir)) {
        let loaded = signer(&chain, key).map(|s| Identity {
            signer: s,
            name: name_of(key).unwrap_or_else(|| GENERATOR.to_string()),
        });
        match loaded.and_then(|id| self_test(&id).map(|()| id)) {
            Ok(id) => return Ok(id),
            // A chain from another key (app data restored from elsewhere,
            // an expired certificate): make a new pair below.
            Err(e) => eprintln!("read aloud: the signing certificate is replaced ({e:#})"),
        }
    }
    let (chain, key, name) = generate().map_err(|e| format!("making the signing key: {e:#}"))?;
    store
        .set(KEY_ACCOUNT, &key)
        .and_then(|()| match store.get(KEY_ACCOUNT)? {
            Some(back) if back == key => Ok(()),
            _ => Err(crate::secrets::StoreError(
                "the key read back from the store does not match".into(),
            )),
        })
        .map_err(|e| {
            format!(
                "the signing key could not be saved in {} ({e})",
                crate::secrets::store_name()
            )
        })?;
    write_chain(dir, &chain).map_err(|e| format!("{e:#}"))?;
    let identity = Identity {
        signer: signer(&chain, &key).map_err(|e| format!("{e:#}"))?,
        name,
    };
    self_test(&identity).map_err(|e| format!("{e:#}"))?;
    Ok(identity)
}

/// The app's identity, loaded once per run from the OS credential store and
/// `dir` (`<app data>/c2pa`). Failures are not kept: a store that works
/// later is used then.
pub fn identity(dir: &Path) -> Result<Arc<Identity>, String> {
    static CACHE: Mutex<Option<(PathBuf, Arc<Identity>)>> = Mutex::new(None);
    let mut slot = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((d, id)) = slot.as_ref() {
        if d == dir {
            return Ok(id.clone());
        }
    }
    let id = Arc::new(load_or_create(dir, &crate::secrets::OsStore)?);
    *slot = Some((dir.to_path_buf(), id.clone()));
    Ok(id)
}

// ---- signing ----------------------------------------------------------------------

/// The manifest of one generated file (see the module docs). `when` is
/// RFC 3339 UTC. Pure.
pub fn manifest_definition(p: &Provenance, when: &str) -> serde_json::Value {
    let agent = serde_json::json!({"name": GENERATOR, "version": env!("CARGO_PKG_VERSION")});
    serde_json::json!({
        "claim_generator_info": [agent.clone()],
        "title": TITLE,
        "assertions": [{
            "label": "c2pa.actions",
            "data": {"actions": [{
                "action": "c2pa.created",
                "digitalSourceType": DIGITAL_SOURCE_TYPE,
                "softwareAgent": agent,
                "when": when,
                "description": format!("Synthetic speech generated by {GENERATOR} ({})", p.engine),
                "parameters": {
                    "engine": p.engine,
                    "voice": p.voice,
                    "language": p.language,
                },
            }]},
        }],
    })
}

/// Now, as the manifest records it.
pub fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Accepts what c2pa writes when it copies a source it doesn't embed into
/// (Ogg), and keeps nothing.
struct Discard(u64);

impl Write for Discard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Read for Discard {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}

impl Seek for Discard {
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Ok(0)
    }
}

fn sign_stream<R: Read + Seek + Send>(
    identity: &Identity,
    p: &Provenance,
    when: &str,
    audio: &mut R,
) -> Result<Vec<u8>> {
    let mut builder = c2pa::Builder::from_context(c2pa::Context::new())
        .with_definition(manifest_definition(p, when))
        .map_err(|e| anyhow!("building the manifest: {e}"))?;
    builder.set_no_embed(true);
    builder
        .sign(identity.signer.as_ref(), OGG_FORMAT, audio, &mut Discard(0))
        .map_err(|e| anyhow!("signing: {e}"))
}

/// The sidecar manifest of the Ogg Opus file `audio` (a data hash over the
/// whole file): the bytes of `<stem>.c2pa`.
pub fn sign_sidecar(
    identity: &Identity,
    p: &Provenance,
    when: &str,
    audio: &Path,
) -> Result<Vec<u8>> {
    let mut f =
        std::fs::File::open(audio).with_context(|| format!("reading {}", audio.display()))?;
    sign_stream(identity, p, when, &mut f)
}

/// `wav` (a whole WAV file) with the manifest embedded.
pub fn embed_in_wav(
    identity: &Identity,
    p: &Provenance,
    when: &str,
    wav: &[u8],
) -> Result<Vec<u8>> {
    let mut builder = c2pa::Builder::from_context(c2pa::Context::new())
        .with_definition(manifest_definition(p, when))
        .map_err(|e| anyhow!("building the manifest: {e}"))?;
    let mut out = Cursor::new(Vec::with_capacity(wav.len() + 8 * 1024));
    builder
        .sign(
            identity.signer.as_ref(),
            "wav",
            &mut Cursor::new(wav),
            &mut out,
        )
        .map_err(|e| anyhow!("signing: {e}"))?;
    Ok(out.into_inner())
}

/// The sidecar of an audio file: the same stem, `.c2pa`.
pub fn sidecar_path(audio: &Path) -> PathBuf {
    audio.with_extension(SIDECAR_EXT)
}

// ---- verification -------------------------------------------------------------------

/// What the signed metadata says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureStatus {
    /// Signed, and the file hasn't changed since. The signer is not on
    /// any trust list (a self-signed certificate, as every Sussurro install
    /// uses).
    Valid,
    /// There is signed metadata, but it doesn't hold: the file changed
    /// after signing, the sidecar belongs to another file, or the manifest
    /// is damaged.
    Invalid,
    /// No signed metadata: nothing embedded and no sidecar.
    None,
}

/// Where the manifest was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureSource {
    /// `<same stem>.c2pa` next to the file.
    Sidecar,
    /// Inside the file.
    Embedded,
}

/// The signed-metadata layer of *Check a file*. Strings are cut to
/// [`MAX_FIELD_CHARS`]; they come from the file, so the UI shows them as
/// text only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignatureLayer {
    pub status: SignatureStatus,
    pub source: Option<SignatureSource>,
    /// The signing certificate's common name and issuer.
    pub signer: String,
    pub issuer: String,
    /// Claim generator (`Sussurro 0.12.0`).
    pub generator: String,
    /// A `c2pa.created` action says `trainedAlgorithmicMedia`.
    pub ai_generated: bool,
    /// The generator is Sussurro. Anyone can sign such a claim with their own
    /// certificate: it says who *claims* to have made the file.
    pub claims_sussurro: bool,
    /// When the manifest says the file was made (the signer's clock).
    pub when: String,
    pub engine: String,
    pub voice: String,
    pub language: String,
    /// Why it is invalid (C2PA status codes, or "can't be read").
    pub problem: String,
}

/// Longest string kept from a manifest.
pub const MAX_FIELD_CHARS: usize = 120;

impl SignatureLayer {
    pub fn none() -> Self {
        Self {
            status: SignatureStatus::None,
            source: None,
            signer: String::new(),
            issuer: String::new(),
            generator: String::new(),
            ai_generated: false,
            claims_sussurro: false,
            when: String::new(),
            engine: String::new(),
            voice: String::new(),
            language: String::new(),
            problem: String::new(),
        }
    }

    fn unreadable(source: SignatureSource) -> Self {
        Self {
            status: SignatureStatus::Invalid,
            source: Some(source),
            problem: "the signed metadata can't be read".into(),
            ..Self::none()
        }
    }
}

fn cut(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_FIELD_CHARS)
        .collect()
}

fn layer_from(reader: &c2pa::Reader, source: SignatureSource) -> SignatureLayer {
    let status = match reader.validation_state() {
        c2pa::ValidationState::Valid | c2pa::ValidationState::Trusted => SignatureStatus::Valid,
        _ => SignatureStatus::Invalid,
    };
    let mut layer = SignatureLayer {
        status,
        source: Some(source),
        ..SignatureLayer::none()
    };
    if status == SignatureStatus::Invalid {
        // The failures, minus the one every self-signed manifest has.
        let codes: Vec<String> = reader
            .validation_status()
            .unwrap_or_default()
            .iter()
            .map(|s| s.code().to_string())
            .filter(|c| c != "signingCredential.untrusted")
            .collect();
        layer.problem = cut(&codes.join(", "));
        if layer.problem.is_empty() {
            layer.problem = "the signature doesn't verify".into();
        }
    }
    let json: serde_json::Value = serde_json::from_str(&reader.json()).unwrap_or_default();
    let active = json["active_manifest"].as_str().unwrap_or_default();
    let m = &json["manifests"][active];
    let s = |v: &serde_json::Value| cut(v.as_str().unwrap_or_default());
    layer.signer = s(&m["signature_info"]["common_name"]);
    layer.issuer = s(&m["signature_info"]["issuer"]);
    let info = &m["claim_generator_info"][0];
    let name = s(&info["name"]);
    let version = s(&info["version"]);
    layer.claims_sussurro = name == GENERATOR;
    layer.generator = if version.is_empty() {
        name
    } else {
        cut(&format!("{name} {version}"))
    };
    let actions = m["assertions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| {
            a["label"]
                .as_str()
                .is_some_and(|l| l == "c2pa.actions" || l.starts_with("c2pa.actions.v"))
        })
        .flat_map(|a| a["data"]["actions"].as_array().into_iter().flatten());
    for a in actions {
        if a["action"] != "c2pa.created" {
            continue;
        }
        layer.ai_generated |= a["digitalSourceType"]
            .as_str()
            .is_some_and(|t| t.ends_with("/trainedAlgorithmicMedia"));
        layer.when = s(&a["when"]);
        layer.engine = s(&a["parameters"]["engine"]);
        layer.voice = s(&a["parameters"]["voice"]);
        layer.language = s(&a["parameters"]["language"]);
    }
    layer
}

/// Check the sidecar manifest `manifest` against the audio `audio`.
pub fn verify_sidecar<R: Read + Seek + Send>(manifest: &[u8], audio: &mut R) -> SignatureLayer {
    if audio.rewind().is_err() {
        return SignatureLayer::unreadable(SignatureSource::Sidecar);
    }
    match c2pa::Reader::from_context(c2pa::Context::new())
        .with_manifest_data_and_stream(manifest, OGG_FORMAT, audio)
    {
        Ok(r) => layer_from(&r, SignatureSource::Sidecar),
        Err(_) => SignatureLayer::unreadable(SignatureSource::Sidecar),
    }
}

/// Formats C2PA embeds in, by file extension (lower case).
pub const EMBEDDED_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a", "flac"];

/// The manifest embedded in `file` (extension `ext`), `None` when the file
/// has none or its format can't hold one.
pub fn verify_embedded<R: Read + Seek + Send>(ext: &str, file: &mut R) -> Option<SignatureLayer> {
    if !EMBEDDED_EXTENSIONS.contains(&ext) || file.rewind().is_err() {
        return None;
    }
    match c2pa::Reader::from_context(c2pa::Context::new()).with_stream(ext, file) {
        Ok(r) => Some(layer_from(&r, SignatureSource::Embedded)),
        Err(
            c2pa::Error::JumbfNotFound
            | c2pa::Error::ProvenanceMissing
            | c2pa::Error::UnsupportedType
            | c2pa::Error::NotFound,
        ) => None,
        Err(_) => Some(SignatureLayer::unreadable(SignatureSource::Embedded)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::secrets::tests::FakeStore;

    fn prov() -> Provenance {
        Provenance {
            engine: "Pocket TTS".into(),
            voice: "giovanni".into(),
            language: "it".into(),
        }
    }

    /// A signing identity for tests: fake store, chain in `dir`.
    pub(crate) fn test_identity(dir: &Path) -> Identity {
        load_or_create(dir, &FakeStore::default()).unwrap()
    }

    #[test]
    fn the_manifest_says_what_made_it_and_nothing_personal() {
        let m = manifest_definition(&prov(), "2026-09-28T10:00:00Z");
        let text = m.to_string();
        assert_eq!(m["claim_generator_info"][0]["name"], GENERATOR);
        assert_eq!(
            m["claim_generator_info"][0]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(m["title"], TITLE);
        let a = &m["assertions"][0]["data"]["actions"][0];
        assert_eq!(a["action"], "c2pa.created");
        assert_eq!(a["digitalSourceType"], DIGITAL_SOURCE_TYPE);
        assert_eq!(a["when"], "2026-09-28T10:00:00Z");
        assert_eq!(a["parameters"]["voice"], "giovanni");
        assert_eq!(a["parameters"]["language"], "it");
        // No soft binding, no ingredients, no paths or names.
        let labels: Vec<&str> = m["assertions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["c2pa.actions"], "no soft binding, nothing else");
        for bad in [
            "soft-binding",
            "ingredient",
            "/",
            "\\",
            "@",
            "transcript",
            "author",
        ] {
            let without_urls = text.replace(DIGITAL_SOURCE_TYPE, "");
            assert!(!without_urls.contains(bad), "{bad} in {text}");
        }
        assert!(now_utc().ends_with('Z'), "UTC, no local offset");
    }

    #[test]
    fn a_sidecar_verifies_and_catches_a_changed_byte_or_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let id = test_identity(&dir.path().join("c2pa"));
        assert!(id.name.starts_with("Sussurro install "), "{}", id.name);
        let audio = dir.path().join("speech.opus");
        let bytes: Vec<u8> = b"OggS"
            .iter()
            .copied()
            .chain((0..40_000u32).map(|i| (i * 7) as u8))
            .collect();
        std::fs::write(&audio, &bytes).unwrap();
        let manifest = sign_sidecar(&id, &prov(), "2026-09-28T10:00:00Z", &audio).unwrap();
        assert!(manifest.len() < 8 * 1024, "{}", manifest.len());

        let ok = verify_sidecar(&manifest, &mut Cursor::new(bytes.clone()));
        assert_eq!(ok.status, SignatureStatus::Valid, "{}", ok.problem);
        assert_eq!(ok.source, Some(SignatureSource::Sidecar));
        assert!(ok.claims_sussurro && ok.ai_generated);
        assert_eq!(ok.signer, id.name);
        assert_eq!(ok.issuer, SUBJECT_ORG);
        assert_eq!(
            ok.generator,
            format!("Sussurro {}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            (ok.engine.as_str(), ok.voice.as_str(), ok.language.as_str()),
            ("Pocket TTS", "giovanni", "it")
        );
        assert_eq!(ok.when, "2026-09-28T10:00:00Z");

        let mut changed = bytes.clone();
        changed[1000] ^= 1;
        let bad = verify_sidecar(&manifest, &mut Cursor::new(changed));
        assert_eq!(bad.status, SignatureStatus::Invalid);
        assert!(bad.problem.contains("dataHash.mismatch"), "{}", bad.problem);

        let other = verify_sidecar(&manifest, &mut Cursor::new(b"OggS another file".to_vec()));
        assert_eq!(other.status, SignatureStatus::Invalid);

        let junk = verify_sidecar(b"not a manifest", &mut Cursor::new(bytes));
        assert_eq!(junk.status, SignatureStatus::Invalid);
        assert!(junk.problem.contains("can't be read"));
    }

    #[test]
    fn a_wav_carries_its_manifest_inside() {
        let dir = tempfile::tempdir().unwrap();
        let id = test_identity(dir.path());
        let wav = crate::tts::engine::wav_bytes(24_000, &vec![0.1; 24_000]);
        let signed = embed_in_wav(&id, &prov(), "2026-09-28T10:00:00Z", &wav).unwrap();
        assert!(signed.len() > wav.len());
        let ok = verify_embedded("wav", &mut Cursor::new(signed.clone())).unwrap();
        assert_eq!(ok.status, SignatureStatus::Valid, "{}", ok.problem);
        assert_eq!(ok.source, Some(SignatureSource::Embedded));
        // The synthetic comment is still there.
        let info = crate::tts::check::read_wav_info(&mut Cursor::new(signed.clone()));
        assert!(info.iter().any(|(k, _)| k == "ICMT"), "{info:?}");
        // A sample changed.
        let mut changed = signed;
        changed[200] ^= 0x40;
        let bad = verify_embedded("wav", &mut Cursor::new(changed)).unwrap();
        assert_eq!(bad.status, SignatureStatus::Invalid);
        // Plain audio: nothing to say.
        assert!(verify_embedded("wav", &mut Cursor::new(wav)).is_none());
        assert!(verify_embedded("opus", &mut Cursor::new(b"OggS".to_vec())).is_none());
    }

    #[test]
    fn the_key_stays_in_the_store_and_the_identity_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let store = FakeStore::default();
        let a = load_or_create(dir.path(), &store).unwrap();
        let key = store.entries.borrow().get(KEY_ACCOUNT).cloned().unwrap();
        assert!(key.contains("PRIVATE KEY"));
        let chain = std::fs::read_to_string(chain_path(dir.path())).unwrap();
        assert!(!chain.contains("PRIVATE"), "only certificates on disk");
        assert_eq!(chain.matches("BEGIN CERTIFICATE").count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(chain_path(dir.path()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(!format!("{a:?}").contains("PRIVATE"));
        // Next run: the same identity, no new write.
        let writes = store.writes.get();
        let b = load_or_create(dir.path(), &store).unwrap();
        assert_eq!(a.name, b.name);
        assert_eq!(store.writes.get(), writes);
        // The chain lost (app data cleared): a new pair.
        std::fs::remove_file(chain_path(dir.path())).unwrap();
        let c = load_or_create(dir.path(), &store).unwrap();
        assert_ne!(a.name, c.name);
        // A chain that doesn't match the key: replaced too.
        std::fs::write(chain_path(dir.path()), chain).unwrap();
        let d = load_or_create(dir.path(), &store).unwrap();
        assert_ne!(d.name, a.name);
    }

    #[test]
    fn without_a_credential_store_nothing_is_signed_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let store = FakeStore::default();
        store.broken.set(true);
        let err = load_or_create(&dir.path().join("c2pa"), &store).unwrap_err();
        assert!(err.contains("not available"), "{err}");
        assert!(
            !dir.path().join("c2pa").exists(),
            "no chain without its key"
        );
        // A store that accepts writes but loses them is not trusted either.
        struct Lossy;
        impl SecretStore for Lossy {
            fn get(&self, _: &str) -> Result<Option<String>, crate::secrets::StoreError> {
                Ok(None)
            }
            fn set(&self, _: &str, _: &str) -> Result<(), crate::secrets::StoreError> {
                Ok(())
            }
            fn delete(&self, _: &str) -> Result<(), crate::secrets::StoreError> {
                Ok(())
            }
        }
        let err = load_or_create(dir.path(), &Lossy).unwrap_err();
        assert!(err.contains("could not be saved"), "{err}");
        assert!(!chain_path(dir.path()).exists());
    }

    #[test]
    fn sidecar_names() {
        assert_eq!(
            sidecar_path(Path::new("/a/speech-notes.opus")),
            Path::new("/a/speech-notes.c2pa")
        );
        assert_eq!(name_of("nonsense"), None);
    }

    /// The real OS credential store, end to end (may prompt):
    /// `cargo test real_signing_key -- --ignored`. Leaves the test entry
    /// removed.
    #[test]
    #[ignore]
    fn real_signing_key_in_the_os_store() {
        struct Scoped;
        impl SecretStore for Scoped {
            fn get(&self, _: &str) -> Result<Option<String>, crate::secrets::StoreError> {
                crate::secrets::OsStore.get("c2pa-signing-key-test-257")
            }
            fn set(&self, _: &str, s: &str) -> Result<(), crate::secrets::StoreError> {
                crate::secrets::OsStore.set("c2pa-signing-key-test-257", s)
            }
            fn delete(&self, _: &str) -> Result<(), crate::secrets::StoreError> {
                crate::secrets::OsStore.delete("c2pa-signing-key-test-257")
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let a = load_or_create(dir.path(), &Scoped).expect("identity");
        let b = load_or_create(dir.path(), &Scoped).expect("reloaded");
        assert_eq!(a.name, b.name);
        Scoped.delete("").unwrap();
    }
}
