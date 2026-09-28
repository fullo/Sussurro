"""Export Meta AudioSeal 0.2 (16-bit base models) to ONNX for Sussurro.

Dev tool, never shipped (E22: no Python at run time). Produces the two files
hosted at https://huggingface.co/DarumaHQ/audioseal-onnx, which the app pins by
revision + SHA-256 (#257, plan E17): the files at revision
55477a4c93a98fd9c38f59173d506c7e9a803589 came from this script (bit-for-bit
the same as spike #240's export), and `sussurro/src-tauri/src/tts/catalog.rs`
pins them.

AudioSeal is MIT licensed, code and weights: Copyright (c) Meta Platforms, Inc.
and affiliates (https://github.com/facebookresearch/audioseal). The export
changes nothing but the format. The message MSG below is the Sussurro payload
the app writes (`tts::watermark::PAYLOAD`, 0xB2E5).

Inputs are pinned: the checkpoints come from `facebook/audioseal` at REVISION
and must match their SHA-256 (MIT, code and weights). Tested with
audioseal 0.2.0, torch 2.14.0 (CPU), onnx 1.23.0, onnxruntime 1.24.2 (the ONNX
Runtime version `ort` 2.0.0-rc.12 links).

    python export_audioseal_onnx.py OUT_DIR

Graphs (opset 17, dynamic batch and length):
- generator: audio [B,1,T] f32 (16 kHz, T a multiple of 320), message [B,16]
  i64 -> watermark [B,1,T] f32, to be added to the audio.
- detector: audio [B,1,T] f32 (16 kHz, T a multiple of 320) -> prob [B,T]
  (per-sample probability of the watermark), bits [B,16] (probability of 1).
The caller pads T up to a multiple of 320 (the SEANet hop, 8*5*4*2) and cuts
the padding off the output.
"""
import hashlib
import json
import os
import sys
import tempfile

import numpy as np
import onnx
import onnxruntime as ort
import torch
import yaml
from huggingface_hub import hf_hub_download

import audioseal
from audioseal import AudioSeal

REPO = "facebook/audioseal"
REVISION = "3c19eba53390776cf2cc9ed5f6c9ac67ce72ecba"
CHECKPOINTS = {
    "audioseal_wm_16bits": (
        "generator_base.pth",
        "7a845b5fbe9364a63a3909d8ab3fe064d13a76ae4c2e983573e08c69b7b51748",
    ),
    "audioseal_detector_16bits": (
        "detector_base.pth",
        "8a78e8a83584113523e161fc599fcab10fd0e94c04d2eb9d2fa1e9ec91ab69d9",
    ),
}
HOP = 320
MSG = torch.tensor([[1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 0, 1, 0, 1]], dtype=torch.int64)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def pinned_card(name, tmp):
    """The package's card with its checkpoint replaced by the pinned, verified file."""
    file, digest = CHECKPOINTS[name]
    path = hf_hub_download(REPO, file, revision=REVISION, local_dir=os.path.join(tmp, "ckpt"))
    got = sha256(path)
    if got != digest:
        sys.exit(f"{file}: SHA-256 {got} != pinned {digest}")
    card_dir = os.path.join(os.path.dirname(audioseal.__file__), "cards")
    with open(os.path.join(card_dir, f"{name}.yaml")) as f:
        card = yaml.safe_load(f)
    card["checkpoint"] = path
    out = os.path.join(tmp, name)
    with open(out + ".yaml", "w") as f:
        yaml.safe_dump(card, f)
    # The loader looks a card up as `<cards dir>/<name>.yaml`; an absolute
    # name without the extension replaces the cards dir.
    return out


class Gen(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, audio, message):
        h = self.m.encoder(audio)
        h = self.m.msg_processor(h, message)
        return self.m.decoder(h)


class Det(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, audio):
        r = self.m.detector(audio)
        prob = torch.softmax(r[:, :2, :], dim=1)[:, 1, :]
        bits = torch.sigmoid(r[:, 2:, :].mean(dim=-1))
        return prob, bits


def main(out_dir):
    torch.manual_seed(0)
    torch.set_num_threads(2)
    os.makedirs(out_dir, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        gen = AudioSeal.load_generator(pinned_card("audioseal_wm_16bits", tmp)).eval()
        det = AudioSeal.load_detector(pinned_card("audioseal_detector_16bits", tmp)).eval()

        # Reference outputs from the unpatched package, on hop-aligned lengths.
        refs = []
        for t in (HOP * 77, 16000 * 9):
            x = torch.randn(1, 1, t) * 0.05
            with torch.no_grad():
                w = gen.get_watermark(x, message=MSG)
                r, bits = det(x + w)
            refs.append((x.numpy(), w.numpy(), r[:, 1].numpy(), bits.numpy()))

        # Trace-friendly padding: with T a multiple of the hop the extra
        # padding is always 0, and unpadding with negative indices keeps the
        # length symbolic in the traced graph (the dynamo exporter fails on
        # the LSTM, so the TorchScript tracer is used).
        import audioseal.libs.moshi.modules.conv as conv

        conv.get_extra_padding_for_conv1d = lambda x, kernel_size, stride, padding_total=0: 0
        conv.unpad1d = lambda x, p: x[..., p[0]:-p[1]] if p[1] > 0 else x[..., p[0]:]

        x = torch.randn(1, 1, 16000 * 3) * 0.05
        with torch.no_grad():
            torch.onnx.export(
                Gen(gen).eval(), (x, MSG), os.path.join(out_dir, "audioseal_generator_16bits.onnx"),
                input_names=["audio", "message"], output_names=["watermark"],
                dynamic_axes={"audio": {0: "B", 2: "T"}, "message": {0: "B"}, "watermark": {0: "B", 2: "T"}},
                opset_version=17, dynamo=False,
            )
            torch.onnx.export(
                Det(det).eval(), (x,), os.path.join(out_dir, "audioseal_detector_16bits.onnx"),
                input_names=["audio"], output_names=["prob", "bits"],
                dynamic_axes={"audio": {0: "B", 2: "T"}, "prob": {0: "B", 1: "T"}, "bits": {0: "B"}},
                opset_version=17, dynamo=False,
            )

    so = ort.SessionOptions()
    so.intra_op_num_threads = 2
    sg = ort.InferenceSession(os.path.join(out_dir, "audioseal_generator_16bits.onnx"), so)
    sd = ort.InferenceSession(os.path.join(out_dir, "audioseal_detector_16bits.onnx"), so)
    parity = []
    for x, w, p, bits in refs:
        wo = sg.run(None, {"audio": x, "message": MSG.numpy()})[0]
        po, bo = sd.run(None, {"audio": (x + wo).astype(np.float32)})
        row = {
            "samples": int(x.shape[-1]),
            "generator_max_abs_diff": float(np.abs(w - wo).max()),
            "detector_prob_max_abs_diff": float(np.abs(p - po).max()),
            "detector_bits_max_abs_diff": float(np.abs(bits - bo).max()),
            "payload_ok": bool(((bo > 0.5).astype(int) == MSG.numpy()).all()),
            "frames_marked": float((po > 0.5).mean()),
        }
        parity.append(row)
        if row["generator_max_abs_diff"] > 1e-4 or row["detector_prob_max_abs_diff"] > 1e-4 or not row["payload_ok"]:
            sys.exit(f"parity check failed: {row}")

    files = {}
    for name in ("audioseal_generator_16bits.onnx", "audioseal_detector_16bits.onnx"):
        path = os.path.join(out_dir, name)
        m = onnx.load(path)
        files[name] = {
            "bytes": os.path.getsize(path),
            "sha256": sha256(path),
            "opset": m.opset_import[0].version,
            "ops": sorted({n.op_type for n in m.graph.node}),
        }
    report = {
        "source": {"repo": REPO, "revision": REVISION,
                   "checkpoints": {f: d for f, d in CHECKPOINTS.values()}},
        "versions": {"audioseal": audioseal.__version__, "torch": torch.__version__,
                     "onnx": onnx.__version__, "onnxruntime": ort.__version__},
        "files": files,
        "parity_vs_unpatched_package": parity,
    }
    with open(os.path.join(out_dir, "export.json"), "w") as f:
        json.dump(report, f, indent=1)
    with open(os.path.join(out_dir, "SHA256SUMS"), "w") as f:
        for name, info in files.items():
            f.write(f"{info['sha256']}  {name}\n")
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
