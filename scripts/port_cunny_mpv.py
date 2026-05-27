#!/usr/bin/env python3
"""Port CuNNy mpv GLSL effects into the SuiSuiView WGSL subset.

This dev-only converter handles the non-dp4a CuNNy mpv shaders whose feature
maps are stored as horizontally/vertically packed luma textures. The generated
WGSL keeps each packed feature cell in a separate source-sized storage texture
so it can reuse the existing SuiSuiView CuNNy runtime path.
"""

from __future__ import annotations

import argparse
import re
from dataclasses import dataclass
from pathlib import Path


COMMON_HEADER = """struct CuNNyParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
};

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var input0_tex: texture_2d<f32>;
@group(0) @binding(2) var input1_tex: texture_2d<f32>;
@group(0) @binding(3) var input2_tex: texture_2d<f32>;
@group(0) @binding(4) var input3_tex: texture_2d<f32>;
@group(0) @binding(5) var input4_tex: texture_2d<f32>;
@group(0) @binding(6) var input5_tex: texture_2d<f32>;
@group(0) @binding(7) var input6_tex: texture_2d<f32>;
@group(0) @binding(8) var input7_tex: texture_2d<f32>;
@group(0) @binding(9) var out0_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(10) var out1_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(11) var out2_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(12) var final_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(13) var<uniform> params: CuNNyParams;

fn clamp_source_coord(coord: vec2<i32>) -> vec2<i32> {
    return clamp(coord, vec2<i32>(0, 0), vec2<i32>(i32(params.source_width) - 1, i32(params.source_height) - 1));
}

fn load_source_luma(coord: vec2<i32>, dx: i32, dy: i32) -> f32 {
    let c = textureLoad(source_tex, clamp_source_coord(coord + vec2<i32>(dx, dy)), 0).rgb;
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}
fn load_input0(coord: vec2<i32>, dx: i32, dy: i32) -> vec4<f32> { return textureLoad(input0_tex, clamp_source_coord(coord + vec2<i32>(dx, dy)), 0); }
fn load_input1(coord: vec2<i32>, dx: i32, dy: i32) -> vec4<f32> { return textureLoad(input1_tex, clamp_source_coord(coord + vec2<i32>(dx, dy)), 0); }
fn load_input2(coord: vec2<i32>, dx: i32, dy: i32) -> vec4<f32> { return textureLoad(input2_tex, clamp_source_coord(coord + vec2<i32>(dx, dy)), 0); }
fn load_input3(coord: vec2<i32>, dx: i32, dy: i32) -> vec4<f32> { return textureLoad(input3_tex, clamp_source_coord(coord + vec2<i32>(dx, dy)), 0); }
fn load_input4(coord: vec2<i32>, dx: i32, dy: i32) -> vec4<f32> { return textureLoad(input4_tex, clamp_source_coord(coord + vec2<i32>(dx, dy)), 0); }
fn load_input5(coord: vec2<i32>, dx: i32, dy: i32) -> vec4<f32> { return textureLoad(input5_tex, clamp_source_coord(coord + vec2<i32>(dx, dy)), 0); }
fn load_input6(coord: vec2<i32>, dx: i32, dy: i32) -> vec4<f32> { return textureLoad(input6_tex, clamp_source_coord(coord + vec2<i32>(dx, dy)), 0); }
fn load_input7(coord: vec2<i32>, dx: i32, dy: i32) -> vec4<f32> { return textureLoad(input7_tex, clamp_source_coord(coord + vec2<i32>(dx, dy)), 0); }

fn sample_source_rgb_for_output(out_coord: vec2<i32>) -> vec3<f32> {
    let source_scale = vec2<f32>(f32(params.source_width) / f32(params.output_width), f32(params.source_height) / f32(params.output_height));
    let source_pos = (vec2<f32>(out_coord) + vec2<f32>(0.5)) * source_scale - vec2<f32>(0.5);
    let p0f = floor(source_pos);
    let t = source_pos - p0f;
    let p0_base = vec2<i32>(i32(p0f.x), i32(p0f.y));
    let p0 = clamp_source_coord(p0_base);
    let p1 = clamp_source_coord(p0_base + vec2<i32>(1, 1));
    let c00 = textureLoad(source_tex, p0, 0).rgb;
    let c10 = textureLoad(source_tex, vec2<i32>(p1.x, p0.y), 0).rgb;
    let c01 = textureLoad(source_tex, vec2<i32>(p0.x, p1.y), 0).rgb;
    let c11 = textureLoad(source_tex, p1, 0).rgb;
    return mix(mix(c00, c10, t.x), mix(c01, c11, t.x), t.y);
}
fn sample_source_luma_for_output(out_coord: vec2<i32>) -> f32 {
    return dot(sample_source_rgb_for_output(out_coord), vec3<f32>(0.299, 0.587, 0.114));
}
fn rgb_to_yuv(rgb: vec3<f32>) -> vec3<f32> { return vec3<f32>(dot(rgb, vec3<f32>(0.299, 0.587, 0.114)), dot(rgb, vec3<f32>(-0.169, -0.331, 0.5)), dot(rgb, vec3<f32>(0.5, -0.419, -0.081))); }
fn yuv_to_rgb(yuv: vec3<f32>) -> vec3<f32> { return vec3<f32>(dot(yuv, vec3<f32>(1.0, -0.00093, 1.401687)), dot(yuv, vec3<f32>(1.0, -0.3437, -0.71417)), dot(yuv, vec3<f32>(1.0, 1.77216, 0.00099))); }
fn write_output_luma_value(out_coord: vec2<i32>, luma: f32) {
    if (out_coord.x >= i32(params.output_width) || out_coord.y >= i32(params.output_height)) { return; }
    let rgb = sample_source_rgb_for_output(out_coord);
    var yuv = rgb_to_yuv(rgb);
    yuv.x = clamp(luma, 0.0, 1.0);
    textureStore(final_tex, out_coord, vec4<f32>(clamp(yuv_to_rgb(yuv), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}

"""


@dataclass(frozen=True)
class PassBlock:
    desc: str
    save: str | None
    binds: list[str]
    width: str
    height: str
    components: str
    body: str
    macro_sources: dict[str, tuple[str, int]]


@dataclass(frozen=True)
class FeatureLayout:
    width_mul: int
    height_mul: int


@dataclass
class ConvertState:
    loaded_samples: set[str]


def parse_passes(text: str) -> list[PassBlock]:
    blocks = re.split(r"(?=//!DESC\s+)", text)
    passes: list[PassBlock] = []
    saved_layouts: dict[str, FeatureLayout] = {}
    for block in blocks:
        if not block.startswith("//!DESC"):
            continue
        desc = re.search(r"//!DESC\s+(.+)", block).group(1).strip()
        save = None
        save_match = re.search(r"//!SAVE\s+(.+)", block)
        if save_match:
            save = save_match.group(1).strip()
        binds = [match.group(1).strip() for match in re.finditer(r"//!BIND\s+(.+)", block)]
        width = required_directive(block, "WIDTH", desc)
        height = required_directive(block, "HEIGHT", desc)
        components = required_directive(block, "COMPONENTS", desc)
        body_match = re.search(r"void\s+hook\(\)\s*\{(.*)\n\}", block, re.S)
        if not body_match:
            raise ValueError(f"Could not parse hook body for {desc}")
        macros: dict[str, tuple[str, int]] = {}
        for define in re.finditer(r"#define\s+(l\d)\(x,\s*y\)\s+(.+)", block):
            macro, expression = define.groups()
            texture = re.search(r"\b([A-Za-z0-9_]+)_raw\b", expression).group(1)
            if texture == "LUMA":
                macros[macro] = ("LUMA", 0)
            else:
                offset_match = re.search(r"\+\s*ivec2\((\d+),\s*(\d+)\)", expression)
                x_offset = int(offset_match.group(1)) if offset_match else 0
                y_offset = int(offset_match.group(2)) if offset_match else 0
                layout = saved_layouts.get(texture)
                if layout is None:
                    raise ValueError(f"Unknown packed feature source {texture} for {desc}")
                macros[macro] = (texture, x_offset + y_offset * layout.width_mul)
        passes.append(PassBlock(desc, save, binds, width, height, components, body_match.group(1), macros))
        if save is not None:
            saved_layouts[save] = feature_layout(width, height)
    return passes


def required_directive(block: str, name: str, desc: str) -> str:
    match = re.search(rf"//!{name}\s+(.+)", block)
    if not match:
        raise ValueError(f"Missing {name} directive for {desc}")
    return match.group(1).strip()


def validate_supported_subset(text: str, passes: list[PassBlock]) -> None:
    lowered = text.lower()
    unsupported_tokens = ("dp4a", "//!offset", "//!hooked")
    for token in unsupported_tokens:
        if token in lowered:
            raise ValueError(f"Unsupported CuNNy mpv shader feature: {token}")
    if len(passes) < 2:
        raise ValueError(f"Expected at least two supported CuNNy mpv passes, got {len(passes)}")
    for index, block in enumerate(passes):
        final_pass = index == len(passes) - 1
        if not final_pass:
            expected_components = "4"
            if block.save is None:
                raise ValueError(f"Expected intermediate SAVE directive for {block.desc}")
        else:
            expected_height = "LUMA.h 2 *"
            expected_components = "1"
            if block.save is not None:
                raise ValueError(f"Expected final pass without SAVE directive for {block.desc}")
        feature_layout(block.width, block.height)
        if final_pass and block.height != expected_height:
            raise ValueError(
                f"Unsupported dimensions/components for {block.desc}: "
                f"{(block.width, block.height, block.components)}, "
                f"expected height/components {(expected_height, expected_components)}"
            )
        if block.components != expected_components:
            raise ValueError(
                f"Unsupported COMPONENTS for {block.desc}: {block.components}, "
                f"expected {expected_components}"
            )
        if not final_pass:
            validate_intermediate_stores(block)


def feature_layout(width: str, height: str) -> FeatureLayout:
    return FeatureLayout(width_multiplier(width), height_multiplier(height))


def width_multiplier(width: str) -> int:
    if width == "LUMA.w":
        return 1
    if width == "LUMA.w 2 *":
        return 2
    if width == "LUMA.w 3 *":
        return 3
    if width == "LUMA.w 4 *":
        return 4
    raise ValueError(f"Unsupported feature WIDTH: {width}")


def height_multiplier(height: str) -> int:
    if height == "LUMA.h":
        return 1
    if height == "LUMA.h 2 *":
        return 2
    raise ValueError(f"Unsupported feature HEIGHT: {height}")


def feature_slot_count(width: str, height: str) -> int:
    layout = feature_layout(width, height)
    return layout.width_mul * layout.height_mul


def validate_intermediate_stores(block: PassBlock) -> None:
    layout = feature_layout(block.width, block.height)
    expected_slots = layout.width_mul * layout.height_mul
    stores = re.findall(r"imageStore\(out_image,\s*opos\s*\+\s*ivec2\((\d+),\s*(\d+)\)", block.body)
    if not stores:
        raise ValueError(f"Expected intermediate imageStore statements for {block.desc}")
    for x_text, y_text in stores:
        x = int(x_text)
        y = int(y_text)
        slot = x + y * layout.width_mul
        if x >= layout.width_mul or y >= layout.height_mul or slot >= expected_slots:
            raise ValueError(
                f"Unsupported output slot for {block.desc}: ivec2({x}, {y}), "
                f"layout {layout.width_mul}x{layout.height_mul}"
            )


def parse_numbers(arg: str) -> list[str]:
    return [value.strip() for value in arg.split(",")]


def dot4_expr(vector: str, matrix_numbers: list[str]) -> str:
    columns = [matrix_numbers[i::4] for i in range(4)]
    return "vec4<f32>(" + ", ".join(
        f"dot({vector}, vec4<f32>({', '.join(column)}))" for column in columns
    ) + ")"


def load_expr(macro: str, dx: str, dy: str, block: PassBlock) -> str:
    source_mode = block.macro_sources.get(macro)
    if source_mode is None:
        layer_match = re.fullmatch(r"l(\d+)", macro)
        feature_sources = [bind for bind in block.binds if bind != "LUMA"]
        if not layer_match or len(feature_sources) != 1:
            raise ValueError(f"Unsupported sample macro {macro} in {block.desc}")
        source = feature_sources[0]
        mode = int(layer_match.group(1))
    else:
        source, mode = source_mode
    x = int(dx) - 1
    y = int(dy) - 1
    if source == "LUMA":
        return f"load_source_luma(coord, {x}, {y})"
    ordered_feature_sources = [bind for bind in block.binds if bind != "LUMA"]
    base = ordered_feature_sources.index(source) * 2
    slot = base + mode
    return f"load_input{slot}(coord, {x}, {y})"


def convert_image_store(
    statement: str,
    block: PassBlock,
    final_pass: bool,
    output_slot_map: dict[int, int],
) -> list[str] | None:
    if not statement.startswith("imageStore("):
        return None
    if final_pass:
        coord = re.search(r"opos\s*\+\s*ivec2\((\d),\s*(\d)\)", statement)
        component = re.search(r"r0\.([xyzw])\s*\+", statement)
        if not coord or not component:
            raise ValueError(f"Unsupported final imageStore: {statement}")
        x, y = coord.groups()
        component_name = component.group(1)
        return [
            f"write_output_luma_value(base + vec2<i32>({x}, {y}), r0.{component_name} + sample_source_luma_for_output(base + vec2<i32>({x}, {y})));"
        ]
    store = re.search(r"opos\s*\+\s*ivec2\((\d),\s*(\d)\).*vec4\((r\d)\)", statement)
    if not store:
        raise ValueError(f"Unsupported intermediate imageStore: {statement}")
    x_text, y_text, register = store.groups()
    layout = feature_layout(block.width, block.height)
    packed_slot = int(x_text) + int(y_text) * layout.width_mul
    if packed_slot not in output_slot_map:
        return []
    binding_slot = output_slot_map[packed_slot]
    return [f"textureStore(out{binding_slot}_tex, coord, {register});"]


def convert_statement(
    statement: str,
    block: PassBlock,
    state: ConvertState,
    final_pass: bool,
    output_slot_map: dict[int, int],
) -> list[str]:
    statement = statement.strip()
    if not statement:
        return []
    skip_prefixes = (
        "#",
        "ivec2 ",
        "for ",
        "int ",
        "if ",
        "barrier()",
        "x < ",
        "x += ",
        "y < ",
        "y += ",
        "}",
        "shared ",
        "F ",
        "vec2 p",
        "p = ",
        "vec2 opt",
        "vec2 fpos",
    )
    sample = re.match(
        r"^(s\d+)_(\d)_(\d)\s*=\s*G\[(\d+)\]\[xy\.y\+(\d+)\]\[xy\.x\+(\d+)\]$",
        statement,
    )
    if sample:
        variable = f"{sample.group(1)}_{sample.group(2)}_{sample.group(3)}"
        layer = sample.group(4)
        dy = sample.group(5)
        dx = sample.group(6)
        expr = load_expr(f"l{layer}", dx, dy, block)
        if variable in state.loaded_samples:
            return [f"{variable} = {expr};"]
        state.loaded_samples.add(variable)
        return [f"var {variable} = {expr};"]

    if "G[" in statement:
        if re.match(r"^G\[\d+\]\[ay\]\[ax\]\s*=", statement):
            return []
        raise ValueError(f"Unsupported CuNNy gather statement: {statement}")

    if (
        statement.startswith(skip_prefixes)
        or "barrier()" in statement
        or statement in {"}", "return"}
    ):
        return []

    image_store = convert_image_store(statement, block, final_pass, output_slot_map)
    if image_store is not None:
        return image_store

    init_decl = re.match(r"^V4\s+(.+)$", statement)
    if init_decl:
        lines = []
        for part in init_decl.group(1).split(","):
            name = part.strip()
            if name.startswith("r"):
                lines.append(f"var {name} = vec4<f32>(0.0);")
        return lines

    zero_init = re.match(r"^(r\d)\s*=\s*V4\(0\.0\)$", statement)
    if zero_init:
        return []

    statement = re.sub(r"\bV4\(", "vec4<f32>(", statement)
    statement = re.sub(
        r"M4\(([^)]*)\)\s*\*\s*(s\d+_\d+_\d+)",
        lambda m: dot4_expr(m.group(2), parse_numbers(m.group(1))),
        statement,
    )
    statement = statement.replace("vec4(r0)", "r0").replace("vec4(r1)", "r1")
    return [statement + ";"]


def output_chunks(block: PassBlock) -> list[list[int]]:
    if block.save is None:
        return [[]]
    slots = list(range(feature_slot_count(block.width, block.height)))
    return [slots[index : index + 3] for index in range(0, len(slots), 3)]


def convert_pass_chunk(
    block: PassBlock,
    index: int,
    prefix: str,
    chunk_index: int,
    chunk_count: int,
    chunk: list[int],
) -> str:
    final_pass = block.save is None
    state = ConvertState(loaded_samples=set())
    output_slot_map = {slot: binding_index for binding_index, slot in enumerate(chunk)}
    entry_point = (
        f"{prefix}_pass_{index}"
        if final_pass or chunk_count == 1
        else f"{prefix}_pass_{index}_chunk_{chunk_index}"
    )
    lines = [
        f"// {block.desc}",
        "@compute @workgroup_size(8, 8)",
        f"fn {entry_point}(@builtin(global_invocation_id) global_id: vec3<u32>) {{",
        "    if (global_id.x >= params.source_width || global_id.y >= params.source_height) { return; }",
        "    let coord = vec2<i32>(i32(global_id.x), i32(global_id.y));",
    ]
    if final_pass:
        lines.append("    let base = coord * vec2<i32>(2, 2);")
    for statement in block.body.split(";"):
        for converted in convert_statement(statement, block, state, final_pass, output_slot_map):
            lines.append(f"    {converted}")
    lines.append("}")
    return "\n".join(lines)


def convert_passes(blocks: list[PassBlock], prefix: str) -> list[str]:
    converted: list[str] = []
    for index, block in enumerate(blocks):
        chunks = output_chunks(block)
        for chunk_index, chunk in enumerate(chunks):
            converted.append(convert_pass_chunk(block, index, prefix, chunk_index, len(chunks), chunk))
    return converted


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--prefix", required=True)
    parser.add_argument("--label", required=True)
    args = parser.parse_args()

    text = args.input.read_text(encoding="utf-8")
    passes = parse_passes(text)
    validate_supported_subset(text, passes)
    output = [
        f"// Generated WGSL port of {args.label}.",
        f"// Source: {args.input.name}.",
        "// Generated by scripts/port_cunny_mpv.py from upstream CuNNy mpv GLSL.",
        "// CuNNy is LGPL-3.0-or-later; keep THIRD_PARTY_NOTICES.txt in sync.",
        "",
        COMMON_HEADER,
    ]
    output.extend(convert_passes(passes, args.prefix))
    args.output.write_text("\n\n".join(output) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
