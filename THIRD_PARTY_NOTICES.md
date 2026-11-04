# Third-party components

The original application code is licensed under MIT, copyright (c) 2026 Chenchen96121. The root `LICENSE` is included in the portable package. This grant does not replace third-party component terms.

## Bundled executables

| Component | Version | License and source |
| --- | --- | --- |
| yt-dlp | 2026.08.19 | [Unlicense and standalone third-party terms](https://github.com/yt-dlp/yt-dlp/tree/2026.08.19); both notices are included under `licenses/`. |
| tiktok-core | 1.1.0 / protocol 1 | Original MIT worker, Apache-2.0 signing code, Python and curl_cffi dependencies. Full terms, NOTICE and pinned source identities are in `licenses/tiktok-core-NOTICES.txt` and `tools/tiktok-core/vendor/SOURCES.json`. |
| Deno | 2.9.7 | [MIT](https://github.com/denoland/deno/tree/v2.9.7); license included under `licenses/`. |
| gallery-dl | 1.32.15 | [GPL-2.0-only](https://codeberg.org/mikf/gallery-dl/src/tag/v1.32.15/LICENSE); exact source and digest record included under `licenses/sources/` and `licenses/`. |
| FFmpeg / FFprobe | 9.0.2-chendownloader-d7643a0fe3 | GPL-3.0-or-later with component-specific terms. [Fixed FFmpeg source](https://github.com/FFmpeg/FFmpeg/tree/d7643a0fe3f4f65516c302eba045ba884faed015); [complete source/build attachment](https://github.com/Chenchen96121/chendownloader/releases/download/1.0.0/chendownloader-ffmpeg-sources-d7643a0fe3.tar.gz). |

The FFmpeg source attachment contains the exact source archives, offline build recipe, compiler image identity, final settings, zimg patch, dependency notices and GCC runtime exception. The archive is 101173495 bytes, SHA-256 `c48e4940c50aa437dd61ff18d84ebaee841c8b7468e93ce08d000c7bd9c6645b`. Preserve access to the corresponding source when redistributing the binaries. The attachment is not an application runtime download.

The portable package retains GPLv3, the FFmpeg source/build location, upstream dependency notices and the GCC Runtime Library Exception under `licenses/`. The runtime exception applies to the relevant compiler runtime components, not unrelated code.

## Application dependencies

- Tauri components use their declared MIT and/or Apache-2.0 terms.
- React uses MIT.
- Exact Rust dependencies are recorded in `src-tauri/Cargo.lock`; JavaScript dependencies are recorded in `package-lock.json`. Their individual terms remain applicable.
- Microsoft Edge WebView2 Runtime is a system prerequisite, not bundled. Its licensing is controlled by Microsoft.

The portable package includes no installer, automatic dependency downloader, account state, browser cookies, user settings, history database or media output.
