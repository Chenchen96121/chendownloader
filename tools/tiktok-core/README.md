# TikTok local core

Private native stdin/JSONL worker for chendownloader. No HTTP server, browser-cookie import, remote signer, or third-party parsing endpoint. The Windows executable is distributed inside the complete portable application.

Build with Python 3.12.14 on Windows x64. `requirements.lock` pins the build and runtime wheels with SHA-256; `wheels.json` identifies their filenames. Install with `python -m pip install --require-hashes -r requirements.lock` (or add `--no-index --find-links <wheel-directory>` for offline builds), then invoke `scripts/build-tiktok-core.ps1 -Python <isolated-python.exe>` from the repository. The script runs unit tests, checks onedir, builds onefile, and writes the real executable hash to the trusted native manifest. The onefile must be launched by the application supervisor with an owned temporary directory and no visible console.

Original worker code is MIT under the repository LICENSE. Files under `vendor/` retain Apache-2.0, NOTICE and source attribution. The signer and the browser-query excerpt originate directly from the pinned upstream commit in SOURCES.json. Bundled Python and wheel dependencies retain their own licenses, which must accompany the final portable release.
