use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatPolicy {
    BuiltIn,
    ExperimentalBuiltIn,
    SystemCodecOnly,
    RestrictedReadOnly,
    Blocked,
}

#[derive(Debug, Clone, Copy)]
pub struct FormatDescriptor {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    pub policy: FormatPolicy,
    pub current_decode: bool,
    pub note: &'static str,
}

impl FormatDescriptor {
    pub fn is_image_page(self) -> bool {
        self.current_decode
            && matches!(
                self.policy,
                FormatPolicy::BuiltIn | FormatPolicy::ExperimentalBuiltIn
            )
    }
}

pub const FORMAT_DESCRIPTORS: &[FormatDescriptor] = &[
    FormatDescriptor {
        name: "JPEG",
        extensions: &["jpg", "jpeg", "jpe", "jfif"],
        policy: FormatPolicy::BuiltIn,
        current_decode: true,
        note: "hot-path built-in decode; scaled JPEG fast path is benchmark-gated",
    },
    FormatDescriptor {
        name: "PNG / APNG",
        extensions: &["png", "apng"],
        policy: FormatPolicy::BuiltIn,
        current_decode: true,
        note: "built-in decode; large static PNG sampling remains benchmark-gated",
    },
    FormatDescriptor {
        name: "GIF",
        extensions: &["gif"],
        policy: FormatPolicy::BuiltIn,
        current_decode: true,
        note: "built-in decode; static large first-frame sampling remains benchmark-gated",
    },
    FormatDescriptor {
        name: "WebP",
        extensions: &["webp"],
        policy: FormatPolicy::BuiltIn,
        current_decode: true,
        note: "built-in decode with optional native-webp libwebp backend",
    },
    FormatDescriptor {
        name: "BMP",
        extensions: &["bmp", "dib"],
        policy: FormatPolicy::BuiltIn,
        current_decode: true,
        note: "built-in decode with direct uncompressed BMP fast path",
    },
    FormatDescriptor {
        name: "TIFF",
        extensions: &["tif", "tiff"],
        policy: FormatPolicy::BuiltIn,
        current_decode: true,
        note: "built-in decode through image crate",
    },
    FormatDescriptor {
        name: "TGA",
        extensions: &["tga"],
        policy: FormatPolicy::BuiltIn,
        current_decode: true,
        note: "built-in decode through image crate",
    },
    FormatDescriptor {
        name: "PNM / PBM / PGM / PPM",
        extensions: &["pnm", "pbm", "pgm", "ppm"],
        policy: FormatPolicy::BuiltIn,
        current_decode: true,
        note: "built-in decode through image crate",
    },
    FormatDescriptor {
        name: "ICO",
        extensions: &["ico"],
        policy: FormatPolicy::BuiltIn,
        current_decode: true,
        note: "built-in decode through image crate; direct ICO picker is available as fallback candidate",
    },
    FormatDescriptor {
        name: "QOI",
        extensions: &["qoi"],
        policy: FormatPolicy::BuiltIn,
        current_decode: true,
        note: "built-in decode through image crate",
    },
    FormatDescriptor {
        name: "DDS",
        extensions: &["dds"],
        policy: FormatPolicy::ExperimentalBuiltIn,
        current_decode: true,
        note: "built-in decode through image crate; texture variants remain compatibility-gated",
    },
    FormatDescriptor {
        name: "OpenEXR",
        extensions: &["exr"],
        policy: FormatPolicy::ExperimentalBuiltIn,
        current_decode: true,
        note: "built-in decode through image crate; high-bit-depth memory limits apply",
    },
    FormatDescriptor {
        name: "Radiance HDR / RGBE",
        extensions: &["hdr", "rgbe"],
        policy: FormatPolicy::ExperimentalBuiltIn,
        current_decode: true,
        note: "built-in decode through image crate; HDR output accuracy is not claimed",
    },
    FormatDescriptor {
        name: "AVIF / Animated AVIF",
        extensions: &["avif"],
        policy: FormatPolicy::ExperimentalBuiltIn,
        current_decode: cfg!(feature = "native-avif"),
        note: "decode-only native-avif libavif/dav1d backend; no encoder and no HEVC fallback",
    },
    FormatDescriptor {
        name: "JPEG XL",
        extensions: &["jxl"],
        policy: FormatPolicy::ExperimentalBuiltIn,
        current_decode: false,
        note:
            "jxl-oxide/libjxl candidate; benchmark and native audit required before default support",
    },
    FormatDescriptor {
        name: "SVG",
        extensions: &["svg", "svgz"],
        policy: FormatPolicy::ExperimentalBuiltIn,
        current_decode: false,
        note: "secure static resvg/usvg candidate; scripts and external resources are blocked",
    },
    FormatDescriptor {
        name: "HEIC / HEIF",
        extensions: &["heic", "heif", "hif"],
        policy: FormatPolicy::SystemCodecOnly,
        current_decode: false,
        note: "Windows WIC/system codec only; no bundled HEVC decoder",
    },
    FormatDescriptor {
        name: "JPEG XR",
        extensions: &["jxr", "wdp", "hdp"],
        policy: FormatPolicy::SystemCodecOnly,
        current_decode: false,
        note: "Windows WIC/system codec preferred",
    },
    FormatDescriptor {
        name: "RAW / DNG",
        extensions: &[
            "dng", "cr2", "cr3", "crw", "nef", "nrw", "orf", "rw2", "pef", "sr2", "arw", "raf",
            "raw",
        ],
        policy: FormatPolicy::SystemCodecOnly,
        current_decode: false,
        note: "Windows WIC/system RAW codec only; LibRaw is not bundled",
    },
    FormatDescriptor {
        name: "RAR / CBR",
        extensions: &["rar", "cbr"],
        policy: FormatPolicy::RestrictedReadOnly,
        current_decode: false,
        note: "read-only restricted candidate; RAR creation is forbidden",
    },
    FormatDescriptor {
        name: "BPG",
        extensions: &["bpg"],
        policy: FormatPolicy::Blocked,
        current_decode: false,
        note: "blocked because BPG is HEVC-based",
    },
    FormatDescriptor {
        name: "CLIP",
        extensions: &["clip"],
        policy: FormatPolicy::Blocked,
        current_decode: false,
        note: "blocked except for a future thumbnail-only preview path",
    },
];

pub const OPENABLE_FILE_EXTENSIONS: &[&str] = &[
    "zip", "cbz", "rar", "cbr", "jpg", "jpeg", "jpe", "jfif", "png", "apng", "webp", "bmp", "dib",
    "gif", "tif", "tiff", "tga", "pnm", "pbm", "pgm", "ppm", "ico", "qoi", "dds", "exr", "hdr",
    "rgbe", "avif", "jxl", "svg", "svgz", "heic", "heif", "hif", "jxr", "wdp", "hdp", "dng", "cr2",
    "cr3", "crw", "nef", "nrw", "orf", "rw2", "pef", "sr2", "arw", "raf", "raw",
];

pub fn descriptor_for_extension(extension: &str) -> Option<&'static FormatDescriptor> {
    let extension = extension.trim_start_matches('.');
    FORMAT_DESCRIPTORS.iter().find(|descriptor| {
        descriptor
            .extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    })
}

pub fn is_image_page_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(descriptor_for_extension)
        .is_some_and(|descriptor| descriptor.is_image_page())
}

pub fn unsupported_message_for_extension(extension: &str) -> Option<String> {
    let descriptor = descriptor_for_extension(extension)?;
    match descriptor.policy {
        FormatPolicy::SystemCodecOnly => Some(format!(
            "{} requires an installed system codec; SuiSuiView does not bundle this decoder.",
            descriptor.name
        )),
        FormatPolicy::RestrictedReadOnly => Some(format!(
            "{} is restricted to read-only support and needs a licensed archive backend before it can be opened.",
            descriptor.name
        )),
        FormatPolicy::Blocked => Some(format!(
            "{} is blocked for commercial distribution safety: {}.",
            descriptor.name, descriptor.note
        )),
        FormatPolicy::ExperimentalBuiltIn if !descriptor.current_decode => Some(format!(
            "{} is an experimental candidate, but no validated decoder is enabled in this build.",
            descriptor.name
        )),
        FormatPolicy::BuiltIn | FormatPolicy::ExperimentalBuiltIn => None,
    }
}

pub fn unsupported_message_for_bytes(bytes: &[u8]) -> Option<&'static str> {
    if is_heif_container(bytes) {
        return Some(
            "HEIC/HEIF requires an installed system HEIF/HEVC codec; SuiSuiView does not bundle an HEVC decoder.",
        );
    }
    if bytes.starts_with(b"BPG") {
        return Some("BPG is blocked because it is HEVC-based.");
    }
    None
}

fn is_heif_container(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let brands = &bytes[8..bytes.len().min(32)];
    if brands
        .chunks(4)
        .any(|brand| matches!(brand, b"avif" | b"avis"))
    {
        return false;
    }
    brands.chunks(4).any(|brand| {
        matches!(
            brand,
            b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        descriptor_for_extension, is_image_page_name, unsupported_message_for_bytes,
        unsupported_message_for_extension, FormatPolicy,
    };

    #[test]
    fn classifies_builtin_and_system_formats() {
        assert_eq!(
            descriptor_for_extension("jpg").map(|format| format.policy),
            Some(FormatPolicy::BuiltIn)
        );
        assert_eq!(
            descriptor_for_extension("exr").map(|format| format.policy),
            Some(FormatPolicy::ExperimentalBuiltIn)
        );
        assert_eq!(
            descriptor_for_extension("heic").map(|format| format.policy),
            Some(FormatPolicy::SystemCodecOnly)
        );
        assert_eq!(
            descriptor_for_extension("rar").map(|format| format.policy),
            Some(FormatPolicy::RestrictedReadOnly)
        );
        assert_eq!(
            descriptor_for_extension("bpg").map(|format| format.policy),
            Some(FormatPolicy::Blocked)
        );
    }

    #[test]
    fn image_page_names_include_system_codec_placeholders() {
        assert!(is_image_page_name("page-001.tiff"));
        assert!(is_image_page_name("texture.dds"));
        assert!(is_image_page_name("plate.exr"));
        assert!(!is_image_page_name("iphone.heic"));
        assert!(!is_image_page_name("candidate.avif"));
        assert!(!is_image_page_name("candidate.jxl"));
        assert!(!is_image_page_name("unsafe.svg"));
        assert!(!is_image_page_name("book.cbr"));
        assert!(!is_image_page_name("unsafe.bpg"));
    }

    #[test]
    fn risky_containers_have_specific_messages() {
        let mut heic = b"\0\0\0\x18ftypheic".to_vec();
        heic.extend_from_slice(&[0; 12]);
        assert!(unsupported_message_for_bytes(&heic)
            .unwrap()
            .contains("HEIC/HEIF"));
        let mut avif = b"\0\0\0\x20ftypavif\0\0\0\0mif1".to_vec();
        avif.extend_from_slice(&[0; 12]);
        assert!(unsupported_message_for_bytes(&avif).is_none());
        assert!(unsupported_message_for_extension("clip")
            .unwrap()
            .contains("blocked"));
    }
}
