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
| Tunable decoders | AutoFast can choose per-format fast paths, while compatible mode keeps the broad `image` baseline. |
| Safe viewing tools | Rotate, flip, invert, smooth, sharpen, and gamma effects are session-only and never rewrite the source image. |
| Optional AI upscale | Real-ESRGAN ncnn-vulkan can be used for the current page when you provide your own local executable and models. |

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
| Built in | Folders, single images, `.zip`, `.cbz`, `.jpg`, `.jpeg`, `.png`, `.apng`, `.webp`, `.bmp`, `.gif`, `.tif`, `.tiff`, `.tga`, `.pnm`, `.pbm`, `.pgm`, `.ppm`, `.ico`, `.qoi`, `.psd` |
| Experimental recognized | `.dds`, `.exr`, `.hdr`, `.rgbe`, `.jxl`, `.svg`; `.avif` is indexed only in `native-avif` builds; `.ai` is indexed only in `native-ai` builds |
| System-codec-only recognized | `.heic`, `.heif`, `.jxr`, RAW/DNG camera formats |

Some formats are intentionally limited or blocked for commercial-distribution
safety. CBR/RAR, ONNX/TensorRT AI backends, page-curl animation,
EXIF/file-info panels, printing, slideshow, external editor integration, and
photo storage boxes are planned or under evaluation for later versions. BPG and
full CLIP parsing are intentionally blocked for v1.

PSD support is view-only. SuiSuiView reads the composite/base image preview
through `zune-psd`; Photoshop layers, blend modes, masks, adjustment layers,
smart objects, and layer effects are not reconstructed. AI support is also
preview-only and requires a PDF-compatible `.ai` file plus a `native-ai` build
with an app-local PDFium library beside the executable. Plain `.pdf`, EPS, PS,
and non-PDF-compatible Illustrator data are not indexed as pages.

## Decoder Backends

The default build keeps native codec risk low: it uses Rust fast paths where
they have been validated, and falls back to the broad `image` crate decoder when
a selected backend cannot decode a page.

AutoFast currently resolves `기본값` like this:

| Format | Default backend |
| --- | --- |
| JPEG | target-aware scaled JPEG when useful, then `zune-jpeg` |
| PNG | large-page sampled PNG when useful, then the `png` crate |
| WebP | `image` baseline for still images, `image-webp` for animation; `libwebp` for still images when built with `native-webp` |
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

- General: delete confirmation, ESC exit, always-on-top, and first/last page
  behavior.
- Image processing: transition effect, CPU resize filter, real-time WGSL
  display upscaling, EXIF orientation, and embedded ICC conversion.
- Decoders: decode mode and per-format decoder choices. `기본값` is shown as
  selected text, with the resolved backend summarized beside each format.
- Performance: automatic or manual page-cache memory, nearby-page prefetch,
  and progressive low-resolution preview.
- View and mouse: large-image starting position, double-click maximize,
  middle-click fullscreen, and wheel behavior.
- Experimental AI upscale: Real-ESRGAN executable path, model name, optional
  model folder, scale, tile size, output format, and optional current/next-page
  AI prefetch.

## Bookmarks And State

Bookmarks and viewer preferences are saved to the platform data directory. On
Windows, this resolves to an AppData `SuiSuiView/state.json` location. The
status bar shows the exact path currently in use.

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
- [x] Optional Real-ESRGAN current-page upscale.
- [x] WGSL display effects and real-time upscaler candidates.
- [x] Per-format decoder benchmarks and user-selectable decoder settings.
- [ ] CBR/RAR read-only archive support after backend and license review.
- [ ] EXIF/file-info panels.
- [ ] Printing, slideshow, and external editor workflows.
- [ ] Broader AI backend options after distribution review.

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

After configuring Real-ESRGAN in settings, use the `AI x4` toolbar button or
the right-click `AI upscale` action to upscale the current page.

The top-bar compare toggle can split the current page into A/B panes. Each side
can use the app default, a CPU resize filter, a WGSL display upscaler, or a
cached AI result.

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
