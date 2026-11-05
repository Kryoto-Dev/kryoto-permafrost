# Third-party code and assets

| What | Used for | Licence |
|---|---|---|
| [preflate-rs](https://github.com/microsoft/preflate-rs) | rebuilding deflate streams exactly | Apache-2.0 |
| [lepton_jpeg](https://github.com/microsoft/lepton_jpeg_rust) | recompressing JPEGs | Apache-2.0 |
| [liblzma](https://github.com/portable-network-archive/liblzma-rs) (XZ Utils) | LZMA2, the x86 and delta filters | MIT or Apache-2.0, 0BSD |
| [zstd](https://github.com/gyscos/zstd-rs) | Frost mode, the manifest | MIT, BSD-3-Clause |
| [ppmd-rust](https://github.com/hasenbanck/ppmd-rust) | text | CC0-1.0 or MIT-0 |
| [fastcdc](https://github.com/nlfiedler/fastcdc-rs) | finding repeated data | MIT |
| [BLAKE3](https://github.com/BLAKE3-team/BLAKE3) | hashes | CC0-1.0 or Apache-2.0 |
| [fontdue](https://github.com/mooman219/fontdue) | text in the setup | MIT or Apache-2.0 or Zlib |
| [Geist Mono](https://github.com/vercel/geist-font) | the setup's typeface, `crates/setup/fonts/` | SIL OFL 1.1 |

Teletext in `public/fonts/` is Kryoto's licensed face and is not covered by this repository's
MIT licence. It is used by the app only. The setup is redistributed by whoever makes a repack,
so it is set in Geist Mono instead.
