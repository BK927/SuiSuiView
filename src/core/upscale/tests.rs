use super::cache::ncnn_realesrgan_cache_key;
use super::{
    build_ncnn_realesrgan_command, normalized_output_format, normalized_tile_size,
    should_check_executable_path,
};
use crate::core::state::NcnnRealEsrganSettings;
use std::path::Path;

#[test]
fn builds_realesrgan_ncnn_command_args() {
    let settings = NcnnRealEsrganSettings {
        executable_path: "C:\\tools\\realesrgan-ncnn-vulkan.exe".to_owned(),
        model_name: "realesrgan-x4plus-anime".to_owned(),
        model_path: "C:\\tools\\models".to_owned(),
        scale: 4,
        tile_size: 0,
        output_format: "png".to_owned(),
    };

    let command = build_ncnn_realesrgan_command(
        &settings,
        Path::new("C:\\tmp\\input.png"),
        Path::new("C:\\tmp\\output.png"),
    )
    .unwrap();

    assert_eq!(
        command.program,
        Path::new("C:\\tools\\realesrgan-ncnn-vulkan.exe")
    );
    assert_eq!(
        command.args,
        vec![
            "-i",
            "C:\\tmp\\input.png",
            "-o",
            "C:\\tmp\\output.png",
            "-n",
            "realesrgan-x4plus-anime",
            "-s",
            "4",
            "-t",
            "0",
            "-f",
            "png",
            "-m",
            "C:\\tools\\models",
        ]
    );
}

#[test]
fn command_builder_requires_executable_and_model() {
    let settings = NcnnRealEsrganSettings::default();
    assert!(build_ncnn_realesrgan_command(
        &settings,
        Path::new("input.png"),
        Path::new("output.png")
    )
    .is_err());

    let settings = NcnnRealEsrganSettings {
        executable_path: "realesrgan-ncnn-vulkan.exe".to_owned(),
        model_name: String::new(),
        ..NcnnRealEsrganSettings::default()
    };
    assert!(build_ncnn_realesrgan_command(
        &settings,
        Path::new("input.png"),
        Path::new("output.png")
    )
    .is_err());
}

#[test]
fn normalizes_tile_and_output_format() {
    assert_eq!(normalized_tile_size(0), 0);
    assert_eq!(normalized_tile_size(1), 32);
    assert_eq!(normalized_tile_size(4096), 2048);
    assert_eq!(normalized_output_format("WEBP"), "webp");
    assert_eq!(normalized_output_format("jpeg"), "jpg");
    assert_eq!(normalized_output_format("unknown"), "png");
}

#[test]
fn cache_key_tracks_realesrgan_output_identity() {
    let base = NcnnRealEsrganSettings {
        model_name: "realesrgan-x4plus-anime".to_owned(),
        output_format: "png".to_owned(),
        ..NcnnRealEsrganSettings::default()
    };
    let mut other_model = base.clone();
    other_model.model_name = "realesrgan-x4plus".to_owned();
    let mut other_format = base.clone();
    other_format.output_format = "webp".to_owned();
    let mut other_path = base.clone();
    other_path.model_path = "C:\\models\\other".to_owned();

    let source_hash = "abc123";
    let base_key = ncnn_realesrgan_cache_key(source_hash, &base);

    assert_eq!(base_key, ncnn_realesrgan_cache_key(source_hash, &base));
    assert_ne!(
        base_key,
        ncnn_realesrgan_cache_key(source_hash, &other_model)
    );
    assert_ne!(
        base_key,
        ncnn_realesrgan_cache_key(source_hash, &other_format)
    );
    assert_ne!(
        base_key,
        ncnn_realesrgan_cache_key(source_hash, &other_path)
    );
}

#[test]
fn executable_precheck_only_applies_to_explicit_paths() {
    assert!(!should_check_executable_path(Path::new(
        "realesrgan-ncnn-vulkan.exe"
    )));
    assert!(should_check_executable_path(Path::new(
        "tools\\realesrgan-ncnn-vulkan.exe"
    )));
    assert!(should_check_executable_path(Path::new(
        "C:\\tools\\realesrgan-ncnn-vulkan.exe"
    )));
}
