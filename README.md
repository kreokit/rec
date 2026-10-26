<p align="center">
  <img src="assets/brand/kreokit-logo-wide.png" alt="KreoKit" width="420">
</p>

# KreoKit Rec

**Free, offline screen and camera recorder.** Record your screen, camera and microphone for tutorials, demos and reaction videos, trim the ends, place your camera bubble, clean up your voice and export a ready-to-share MP4, with no account and no upload.

Part of [KreoKit](https://github.com/kreokit), free open-source creator tools.

> **Status:** in development. Not ready to download yet.

## Development

Needs Node 22+, pnpm 10, Rust (stable) and the [Tauri 2 prerequisites](https://tauri.app/start/prerequisites/).

```bash
pnpm install
pnpm tauri dev       # the app, with the interface on http://localhost:1490
pnpm test            # interface tests
cd src-tauri && cargo test
```

## Licence

KreoKit Rec is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Third-party components and their licences are listed in [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).
