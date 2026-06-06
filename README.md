<div align="center">
  <img src="./assets/app-icon.png" alt="SuiSuiView app icon" width="96" height="96">
  <h1>SuiSuiView</h1>
  <p><strong>Fast, lightweight native image and comic viewer for folders, ZIP, and CBZ.</strong></p>
  <p>
    <img alt="Rust" src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white">
    <img alt="egui" src="https://img.shields.io/badge/egui-native%20UI-4B5563">
    <img alt="wgpu" src="https://img.shields.io/badge/wgpu-ready-2563EB">
    <img alt="Status" src="https://img.shields.io/badge/status-alpha-orange">
    <img alt="License" src="https://img.shields.io/badge/license-GPL--3.0--only-blue">
  </p>
</div>

SuiSuiView is built for people who want image folders and comic archives to open
quickly, turn pages smoothly, and remember their place without feeling like a
full media-management suite.

![SuiSuiView main window](./assets/preview.png)

## Why SuiSuiView?

| Focus | What it means |
| --- | --- |
| Fast reading | Background decode, display-sized preparation, nearby-page cache, and lightweight transitions. |
| Comic friendly | Folder, `.zip`, and `.cbz` support with single-page and two-page spread modes. |
| Stable bookmarks | ZIP/CBZ bookmarks are based on book contents, so moving or renaming a book should keep your place. |
| Tunable decoders | Auto Fast uses the app default fast paths, compatibility mode keeps the broad `image` baseline, and custom mode lets you override per-format decoders. |
| Tunable scaling | Separate CPU and WGPU scaler choices let display preparation stay light while GPU rendering can use quality-first downscaling. |
| Safe viewing tools | Rotate, flip, invert, smooth, sharpen, and gamma effects are session-only and never rewrite the source image. |

## Quick Start

If you are running SuiSuiView from the source tree:

```powershell
cargo run --release
```

Release builds are the right way to judge page-turn and image-decode
responsiveness.

## Open And Read

- `F2` or `Ctrl+O`: open a file.
- `F`: open a folder.
- Drag and drop: open a file or folder directly.
- `PgDn`, `Down`, `Right`, or `Space`: next page.
- `PgUp`, `Up`, `Left`, `Backspace`, or `Shift+Space`: previous page.
- `1`, `9`, or `Z`: fit page.
- `8`: fit width.
- `7`: two-page left-to-right.
- `6`: two-page right-to-left.
- `F5`: settings.
- `F1`: app information and third-party open-source notices.

Right-click opens a context menu with common open, navigation, view, processing,
delete, copy, and window actions.

## Supported Files

| Tier | Formats |
| --- | --- |
| Built in | Folders, single images, `.zip`, `.cbz`, `.jpg`, `.jpeg`, `.jpe`, `.jfif`, `.png`, `.apng`, `.webp`, `.bmp`, `.dib`, `.gif`, `.tif`, `.tiff`, `.tga`, `.pnm`, `.pbm`, `.pgm`, `.ppm`, `.ico`, `.qoi`, `.psd` |
| Experimental recognized | `.dds`, `.exr`, `.hdr`, `.rgbe`, `.jxl`, `.svg`, `.svgz`; `.avif` is indexed only in `native-avif` builds; `.ai` is indexed only in `native-ai` builds |
| Recognized but not decoded yet | `.heic`, `.heif`, `.jxr`, RAW/DNG camera formats; SuiSuiView shows a format-specific message instead of opening them until a system-codec backend exists |

Some formats are intentionally limited or blocked for commercial-distribution
safety. CBR/RAR, page-curl animation, printing, slideshow, external editor
integration, and photo storage boxes are planned or under evaluation for later
versions. BPG and full CLIP parsing are intentionally blocked for v1.

PSD support is view-only. SuiSuiView reads the composite/base image preview
through `zune-psd`; Photoshop layers, blend modes, masks, adjustment layers,
smart objects, and layer effects are not reconstructed. Adobe Illustrator
`.ai` support is also preview-only and requires a PDF-compatible `.ai` file
plus a `native-ai` build with an app-local PDFium library beside the
executable. Plain `.pdf`, EPS, PS, and non-PDF-compatible Illustrator data are
not indexed as pages.

## Decoder Backends

The default build keeps native codec risk low: it uses Rust fast paths where
they have been validated, and falls back to the broad `image` crate decoder when
a selected backend cannot decode a page.

Auto Fast uses these default decoder paths. Custom mode uses the same path when
a format is set to `기본값`:

| Format | Default backend |
| --- | --- |
| JPEG | target-aware scaled JPEG when useful, then `zune-jpeg` |
| PNG | large-page sampled PNG when useful, then the `png` crate |
| WebP | `image` baseline for still images, `image-webp` first-frame decode for animated WebP; `libwebp` for still images when built with `native-webp` |
| GIF | large static GIF sampling when useful, then the `gif` crate first-frame path |
| BMP | sampled BMP when useful, then direct 24/32-bit BMP fast path |
| ICO | `image` baseline by default, with an explicit ICO fast-path option |
| AVIF | `libavif + dav1d` only when built with `native-avif` |
| SVG | shown in settings as planned, but not enabled for viewing yet |
| PSD | `zune-psd` composite/base image preview |
| AI | PDFium first-page preview only when built with `native-ai` |

Optional native features are explicit build choices:

```powershell
cargo run --release --features native-webp
uv run --with meson --with ninja cargo run --release --features native-avif
cargo run --release --features native-ai
```

Native here means Rust calls an external C/assembly codec library through a Rust
wrapper. `native-webp` uses libwebp, and `native-avif` uses libavif with dav1d.
`native-ai` uses `pdfium-render` to call an app-local PDFium dynamic library.
They are not enabled in the default build, and release bundles that enable them
must carry the notices and update policy recorded in `THIRD_PARTY_NOTICES.txt`.

For AI preview development builds, fetch a V8/XFA-free PDFium package and copy
the platform library next to the executable:

```powershell
uv run python scripts\fetch_pdfium.py --platform windows-x64 --copy-to target\release
```

For release packaging, pass the expected archive checksum with `--sha256`; the
script also prints the downloaded archive checksum for provenance records.

Benchmark-only native candidates such as TurboJPEG remain out of the production
settings.

## Settings

`F5` opens the settings window. Settings are saved with bookmarks in the app
state file.

- General: UI language, delete confirmation, ESC exit, always-on-top, and
  first/last page behavior.
- Rendering: transition effect, CPU up/down scaler filters, GPU acceleration,
  GPU upscaler, WGPU downscaler, EXIF orientation, embedded ICC conversion,
  prefetch, and cache memory.
  CPU preparation has separate upscaler and downscaler filters. The CPU
  upscaler defaults to CatmullRom, and the CPU downscaler defaults to Hamming
  for balanced preparation cost.
  WGPU display downscaling is separate from CPU preparation. It applies when
  the WGPU renderer shrinks a prepared texture for display, and it does not
  change the prepared-page cache. Existing single-pass filters remain available,
  and the WGPU default is Pyramid + Lanczos3 for quality-first shrink. That
  default can cost more GPU time than simpler filters such as Hamming or
  Bilinear.
  WGPU downscaler choices include Hardware Mipmap Linear, Pyramid Box/Tent,
  Pyramid + Hamming, Pyramid + Mitchell, Pyramid + Lanczos2, and Pyramid +
  Lanczos3.
  GPU upscaler `Auto` is content-aware only for enlargement: confident webtoon,
  anime, and manga pages use Anime4K M, while photos and uncertain images keep
  the FSR fallback.
  When GPU acceleration is enabled, SuiSuiView uses the WGPU fast-start handoff
  path by default. If that startup handoff fails, the app falls back to normal
  mode, turns GPU acceleration off, and shows a report dialog with a diagnostic
  log location.
  GPU upscalers marked `(실험)` are selectable for local testing but are not
  treated as stable defaults; SR Lab SPAN also requires a local manifest.
- Decoders: decode mode and per-format decoder choices. `기본값` is shown as
  selected text, with the resolved backend summarized beside each format.
- View, keyboard, and mouse: large-image starting position, visible viewer UI,
  customizable keyboard shortcuts, double-click maximize, middle-click
  fullscreen, and wheel behavior.

The UI language can be set to system default, Korean, or English. UI text and
state words such as Default, Off, and Experimental are localized, while
algorithm, model, codec, library, and file-format names such as ArtCNN,
libwebp, PDFium, JPEG, and ZIP/CBZ keep their English technical names.

## Bookmarks And State

Bookmarks and viewer preferences are saved to the platform data directory. On
Windows, this resolves to an AppData `SuiSuiView/state.json` location.

For ZIP and CBZ files, the bookmark key is based on archive contents, not the
archive path. Moving or renaming the same archive should keep the bookmark.

- `B`: toggle a bookmark for the current page.
- `Ctrl+B`: open the bookmark popover.

View effects are intentionally not saved. Opening or closing a book resets
rotation, flips, filters, gamma, and inversion.

## Delete And Clipboard

- `Delete`: move the current file to the Recycle Bin.
- `Shift+Delete`: permanently delete after confirmation.
- `Ctrl+Enter`: reveal the current file.
- `Ctrl+C`: copy the current page image.
- `Ctrl+Alt+C`: copy the visible spread image.
- `Ctrl+Alt+Shift+C`: copy the page path.

Delete actions operate on real files only. For ZIP and CBZ, the delete target is
the whole archive file, never an internal page. For folders and single images,
the target is the current image file.

For ZIP and CBZ books, copied page paths use a virtual form such as
`book.cbz::chapter/page001.jpg` because archive-internal pages are not
standalone files.

## Roadmap

- [x] Native image and comic viewer.
- [x] Folder, ZIP, and CBZ support.
- [x] Path-independent ZIP/CBZ bookmarks.
- [x] Large-image preview, cache, and display preparation policy.
- [x] WGSL display effects and real-time upscaler candidates.
- [x] Content-aware Auto display upscaling for drawn pages.
- [x] Separate CPU/WGPU scaler controls with quality-first WGPU pyramid
  downscaling.
- [x] Per-format decoder benchmarks and user-selectable decoder settings.
- [x] Current-page EXIF/file/color information panel.
- [ ] CBR/RAR read-only archive support after backend and license review.
- [ ] Printing, slideshow, and external editor workflows.
- [ ] SR Lab research-model path for RFDN, RepRFN, and SPAN/SPANV2 after
  redistributable weights, model conversion, and WGSL tiny-net runtime proof.

<details>
<summary>Keyboard And Mouse Reference</summary>

### Open And Window

- `F2` or `Ctrl+O`: open a file.
- `F`: open a folder.
- `F4`: close the current book.
- `Esc`, `X`, or `Ctrl+W`: exit.
- `F11`, `Alt+Enter`, or `N`: fullscreen.
- `M`: maximize or restore.
- `Q`: minimize.
- `Ctrl+A`: always on top.

### Page Movement

- `PgDn`, `Down`, `Right`, or `Space`: next page.
- `PgUp`, `Up`, `Left`, `Backspace`, or `Shift+Space`: previous page.
- `Home` / `End`: first or last page.
- `Ctrl+PgDn` / `Ctrl+PgUp`: jump 10 pages.
- `Ctrl+Shift+Right` / `Ctrl+Shift+Left`: jump 100 pages.
- `Shift+PgDn` / `Shift+PgUp`: force a one-page move in two-page mode.
- `Ctrl+Alt+PgDn` / `Ctrl+Alt+PgUp`: random page.
- `]` / `[`: open the next or previous folder, ZIP, or CBZ beside the current
  book.

### View

- `0` or `*`: original size.
- `1`, `9`, or `Z`: fit page.
- `8`: fit width.
- `7`: two-page left-to-right.
- `6`: two-page right-to-left.
- `2`: toggle two-page mode.
- The view selector also includes fit height.
- `+` / `-`: zoom in or out.
- `Ctrl++` / `Ctrl+-`: change zoom by 1%.

### Display Effects

- `Ctrl+I`: invert colors.
- `Ctrl+M`: flip horizontal.
- `Ctrl+F`: flip vertical.
- `Ctrl+L` / `Ctrl+R`: rotate left or right.
- `Alt+Up`, `Alt+Left`, `Alt+Right`, `Alt+Down`: set rotation.
- `U`, `I`, `S`: change display filter.
- `Ctrl+G`: toggle gamma correction.

The top-bar compare toggle can split the current page into A/B panes. Each side
can use the app default, CPU scaler filters, or a WGSL display upscaler.

### Mouse

- Drag to pan.
- Mouse wheel moves pages by default.
- `Ctrl+mouse wheel`: zoom.
- Double-click: maximize or restore.
- Middle-click: fullscreen.
- `Ctrl+middle-click`: return to 100%.

</details>

## License

SuiSuiView is licensed under `GPL-3.0-only`. The free GitHub release and the
paid Microsoft Store release are built around the same open-source viewer; the
Store release pays for official Store distribution, signed installation, and
Microsoft Store automatic updates.
