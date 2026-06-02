use crate::core::i18n::I18n;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecodeMode {
    #[default]
    AutoFast,
    Compatibility,
    Custom,
}

impl DecodeMode {
    pub const ALL: [Self; 3] = [Self::AutoFast, Self::Compatibility, Self::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Self::AutoFast => "Auto Fast",
            Self::Compatibility => "호환성 우선",
            Self::Custom => "커스텀",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::AutoFast => i18n.text("label.decode.auto_fast"),
            Self::Compatibility => i18n.text("label.decode.compatibility"),
            Self::Custom => i18n.text("label.decode.custom"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DecoderPreference {
    #[default]
    Default,
    ImageCrate,
    ZuneJpeg,
    PngCrate,
    ZunePng,
    ImageWebp,
    LibWebp,
    GifCrate,
    BmpFastPath,
    IcoFastPath,
    LibAvifDav1d,
    Resvg,
    ZunePsd,
    PdfiumAi,
}

impl DecoderPreference {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "기본값",
            Self::ImageCrate => "image crate",
            Self::ZuneJpeg => "zune-jpeg",
            Self::PngCrate => "png crate",
            Self::ZunePng => "zune-png",
            Self::ImageWebp => "image-webp",
            Self::LibWebp => "libwebp",
            Self::GifCrate => "gif crate",
            Self::BmpFastPath => "BMP fast path",
            Self::IcoFastPath => "ICO fast path",
            Self::LibAvifDav1d => "libavif + dav1d",
            Self::Resvg => "resvg",
            Self::ZunePsd => "zune-psd",
            Self::PdfiumAi => "PDFium",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::Default => i18n.text("state.default"),
            _ => self.label().to_owned(),
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::ImageCrate => "image",
            Self::ZuneJpeg => "zune-jpeg",
            Self::PngCrate => "png",
            Self::ZunePng => "zune-png",
            Self::ImageWebp => "image-webp",
            Self::LibWebp => "libwebp",
            Self::GifCrate => "gif",
            Self::BmpFastPath => "bmp-fast",
            Self::IcoFastPath => "ico-fast",
            Self::LibAvifDav1d => "libavif-dav1d",
            Self::Resvg => "resvg",
            Self::ZunePsd => "zune-psd",
            Self::PdfiumAi => "pdfium-ai",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DecoderPreferences {
    #[serde(default)]
    pub jpeg: DecoderPreference,
    #[serde(default)]
    pub png: DecoderPreference,
    #[serde(default)]
    pub webp: DecoderPreference,
    #[serde(default)]
    pub gif: DecoderPreference,
    #[serde(default)]
    pub bmp: DecoderPreference,
    #[serde(default)]
    pub ico: DecoderPreference,
    #[serde(default)]
    pub avif: DecoderPreference,
    #[serde(default)]
    pub svg: DecoderPreference,
    #[serde(default)]
    pub psd: DecoderPreference,
    #[serde(default)]
    pub ai: DecoderPreference,
}

impl Default for DecoderPreferences {
    fn default() -> Self {
        Self {
            jpeg: DecoderPreference::Default,
            png: DecoderPreference::Default,
            webp: DecoderPreference::Default,
            gif: DecoderPreference::Default,
            bmp: DecoderPreference::Default,
            ico: DecoderPreference::Default,
            avif: DecoderPreference::Default,
            svg: DecoderPreference::Default,
            psd: DecoderPreference::Default,
            ai: DecoderPreference::Default,
        }
    }
}

impl DecoderPreferences {
    pub fn cache_token(self) -> String {
        format!(
            "jpeg:{}-png:{}-webp:{}-gif:{}-bmp:{}-ico:{}-avif:{}-svg:{}-psd:{}-ai:{}",
            self.jpeg.token(),
            self.png.token(),
            self.webp.token(),
            self.gif.token(),
            self.bmp.token(),
            self.ico.token(),
            self.avif.token(),
            self.svg.token(),
            self.psd.token(),
            self.ai.token()
        )
    }
}
