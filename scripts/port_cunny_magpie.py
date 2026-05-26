#!/usr/bin/env python3
"""Port CuNNy Magpie HLSL effects into the SuiSuiView WGSL subset.

This is a dev-only converter for the normal NVL CuNNy Magpie shaders. It keeps
the generated WGSL traceable to the upstream HLSL and avoids hand-copying large
weight blocks.
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
fn load_source_rgb(coord: vec2<i32>, dx: i32, dy: i32) -> vec3<f32> {
    return textureLoad(source_tex, clamp_source_coord(coord + vec2<i32>(dx, dy)), 0).rgb;
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
fn rgb_to_yuv(rgb: vec3<f32>) -> vec3<f32> { return vec3<f32>(dot(rgb, vec3<f32>(0.299, 0.587, 0.114)), dot(rgb, vec3<f32>(-0.169, -0.331, 0.5)), dot(rgb, vec3<f32>(0.5, -0.419, -0.081))); }
fn yuv_to_rgb(yuv: vec3<f32>) -> vec3<f32> { return vec3<f32>(dot(yuv, vec3<f32>(1.0, -0.00093, 1.401687)), dot(yuv, vec3<f32>(1.0, -0.3437, -0.71417)), dot(yuv, vec3<f32>(1.0, 1.77216, 0.00099))); }
fn write_output_luma_delta(out_coord: vec2<i32>, delta: f32) {
    if (out_coord.x >= i32(params.output_width) || out_coord.y >= i32(params.output_height)) { return; }
    let rgb = sample_source_rgb_for_output(out_coord);
    var yuv = rgb_to_yuv(rgb);
    yuv.x = clamp(yuv.x + delta, 0.0, 1.0);
    textureStore(final_tex, out_coord, vec4<f32>(clamp(yuv_to_rgb(yuv), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}
fn write_output_rgb_delta(out_coord: vec2<i32>, delta: vec3<f32>) {
    if (out_coord.x >= i32(params.output_width) || out_coord.y >= i32(params.output_height)) { return; }
    textureStore(final_tex, out_coord, vec4<f32>(clamp(sample_source_rgb_for_output(out_coord) + delta, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}

"""


@dataclass(frozen=True)
class PassBlock:
    desc: str
    inputs: list[str]
    outputs: list[str]
    body: str
    macro_sources: dict[str, tuple[str, str]]


@dataclass
class PassConvertState:
    loaded_samples: set[str]


def parse_passes(text: str) -> list[PassBlock]:
    passes: list[PassBlock] = []
    for match in re.finditer(r"//!PASS\s+\d+(.*?)(?=//!PASS\s+\d+|\Z)", text, re.S):
        block = match.group(0)
        desc = re.search(r"//!DESC\s+(.+)", block).group(1).strip()
        inputs = [part.strip() for part in re.search(r"//!IN\s+(.+)", block).group(1).split(",")]
        outputs = [part.strip() for part in re.search(r"//!OUT\s+(.+)", block).group(1).split(",")]
        body_match = re.search(r"void\s+Pass\d+\([^)]*\)\s*\{(.*)\n\}", block, re.S)
        if not body_match:
            raise ValueError(f"Could not parse body for pass {desc}")
        macro_sources: dict[str, tuple[str, str]] = {}
        for define in re.finditer(r"#define\s+(L\d)\(x, y\)\s+(.+)", block):
            macro, expression = define.groups()
            texture_match = re.search(r"O\((INPUT|T\d+),", expression)
            if not texture_match:
                raise ValueError(f"Could not parse {macro} source in pass {desc}")
            texture = texture_match.group(1)
            if texture == "INPUT" and ".rgb" in expression and "dot(" not in expression:
                mode = "rgb"
            elif texture == "INPUT":
                mode = "luma"
            else:
                mode = "vec4"
            macro_sources[macro] = (texture, mode)
        passes.append(PassBlock(desc, inputs, outputs, body_match.group(1), macro_sources))
    return passes


def parse_numbers(arg: str) -> list[str]:
    return [value.strip() for value in arg.split(",")]


def dot4_expr(vector: str, matrix_numbers: list[str]) -> str:
    columns = [matrix_numbers[i::4] for i in range(4)]
    return "vec4<f32>(" + ", ".join(
        f"dot({vector}, vec4<f32>({', '.join(column)}))" for column in columns
    ) + ")"


def dot3x4_expr(vector: str, matrix_numbers: list[str]) -> str:
    columns = [matrix_numbers[i::4] for i in range(4)]
    return "vec4<f32>(" + ", ".join(
        f"dot({vector}, vec3<f32>({', '.join(column)}))" for column in columns
    ) + ")"


def load_expr(macro: str, dx: str, dy: str, block: PassBlock) -> str:
    source, mode = block.macro_sources[macro]
    x = int(float(dx))
    y = int(float(dy))
    if source == "INPUT":
        if mode == "rgb":
            return f"load_source_rgb(coord, {x}, {y})"
        return f"load_source_luma(coord, {x}, {y})"
    intermediate_inputs = [name for name in block.inputs if name != "INPUT"]
    slot = intermediate_inputs.index(source)
    return f"load_input{slot}(coord, {x}, {y})"


def convert_statement(statement: str, block: PassBlock, state: PassConvertState) -> list[str]:
    statement = statement.strip()
    if not statement:
        return []
    skip_prefixes = (
        "#define ",
        "float2 pt",
        "uint2 gxy",
        "uint2 sz",
        "float2 pos",
        "static const",
        "float2 opt",
        "float2 fpos",
        "float3 yuv",
        "yuv =",
        "OUTPUT[",
    )
    if statement.startswith(skip_prefixes) or statement == "return" or statement.startswith("if "):
        return []
    if re.match(r"^(min16float|V3|V4)\s+s", statement):
        return []

    init = re.match(r"^V4\s+(.+)$", statement)
    if init:
        lines = []
        for part in init.group(1).split(","):
            name_value = part.strip()
            if not name_value:
                continue
            name = name_value.split("=")[0].strip()
            if name.startswith("r"):
                lines.append(f"var {name} = vec4<f32>(0.0);")
        return lines

    load = re.match(r"^(s\d+_\d+_\d+)\s*=\s*(L\d)\((-?\d+\.0),\s*(-?\d+\.0)\)$", statement)
    if load:
        name, macro, dx, dy = load.groups()
        expr = load_expr(macro, dx, dy, block)
        if name in state.loaded_samples:
            return [f"{name} = {expr};"]
        state.loaded_samples.add(name)
        return [f"var {name} = {expr};"]

    statement = re.sub(r"\bV4\(", "vec4<f32>(", statement)
    statement = re.sub(r"\bV3\(", "vec3<f32>(", statement)

    statement = re.sub(
        r"mul\(([^,]+),\s*M4\(([^)]*)\)\)",
        lambda m: dot4_expr(m.group(1).strip(), parse_numbers(m.group(2))),
        statement,
    )
    statement = re.sub(
        r"mul\(([^,]+),\s*M3x4\(([^)]*)\)\)",
        lambda m: dot3x4_expr(m.group(1).strip(), parse_numbers(m.group(2))),
        statement,
    )
    statement = re.sub(
        r"^(r\d+)\s*=\s*max\(\1,\s*0\.0\)$",
        r"\1 = max(\1, vec4<f32>(0.0))",
        statement,
    )

    store = re.match(r"^(T\d+)\[gxy\]\s*=\s*(r\d+)$", statement)
    if store:
        texture, value = store.groups()
        if texture not in block.outputs:
            raise ValueError(f"{texture} is not an output of pass {block.desc}")
        slot = block.outputs.index(texture)
        return [f"textureStore(out{slot}_tex, coord, {value});"]

    return [statement + ";"]


def split_statements(body: str) -> list[str]:
    statements = []
    for raw_line in body.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        parts = line.split(";")
        for part in parts[:-1]:
            statements.append(part)
        if parts[-1].strip():
            statements.append(parts[-1])
    return statements


def final_write_lines(block: PassBlock) -> list[str]:
    has_rgb_delta = "float3(r0.x, r1.x, r2.x)" in block.body
    if has_rgb_delta:
        return [
            "let base = coord * vec2<i32>(2, 2);",
            "write_output_rgb_delta(base + vec2<i32>(0, 0), vec3<f32>(r0.x, r1.x, r2.x));",
            "write_output_rgb_delta(base + vec2<i32>(1, 0), vec3<f32>(r0.y, r1.y, r2.y));",
            "write_output_rgb_delta(base + vec2<i32>(0, 1), vec3<f32>(r0.z, r1.z, r2.z));",
            "write_output_rgb_delta(base + vec2<i32>(1, 1), vec3<f32>(r0.w, r1.w, r2.w));",
        ]
    return [
        "let base = coord * vec2<i32>(2, 2);",
        "write_output_luma_delta(base + vec2<i32>(0, 0), r0.x);",
        "write_output_luma_delta(base + vec2<i32>(1, 0), r0.y);",
        "write_output_luma_delta(base + vec2<i32>(0, 1), r0.z);",
        "write_output_luma_delta(base + vec2<i32>(1, 1), r0.w);",
    ]


def generate_pass(prefix: str, index: int, block: PassBlock) -> str:
    lines = [f"// {block.desc}"]
    lines.append("@compute @workgroup_size(8, 8)")
    lines.append(f"fn {prefix}_pass_{index}(@builtin(global_invocation_id) global_id: vec3<u32>) {{")
    lines.append("    if (global_id.x >= params.source_width || global_id.y >= params.source_height) { return; }")
    lines.append("    let coord = vec2<i32>(i32(global_id.x), i32(global_id.y));")
    state = PassConvertState(loaded_samples=set())
    for statement in split_statements(block.body):
        for converted in convert_statement(statement, block, state):
            lines.append(f"    {converted}")
    if "OUTPUT" in block.outputs:
        for line in final_write_lines(block):
            lines.append(f"    {line}")
    lines.append("}")
    return "\n".join(lines)


def generate(source: Path, prefix: str, label: str) -> str:
    text = source.read_text(encoding="utf-8")
    passes = parse_passes(text)
    result = [
        f"// Generated WGSL port of {label}.",
        f"// Source: {source.name}.",
        "// Generated by scripts/port_cunny_magpie.py from upstream CuNNy Magpie HLSL.",
        "// CuNNy is LGPL-3.0-or-later in the repository license, while the Magpie",
        "// effect headers grant GPL-3.0-or-later terms. SuiSuiView is GPL-3.0-only;",
        "// see THIRD_PARTY_NOTICES.txt for attribution and source availability notes.",
        "",
        COMMON_HEADER,
    ]
    for index, block in enumerate(passes):
        result.append(generate_pass(prefix, index, block))
        result.append("")
    return "\n".join(result).rstrip() + "\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--prefix", required=True)
    parser.add_argument("--label", required=True)
    args = parser.parse_args()
    args.output.write_text(generate(args.source, args.prefix, args.label), encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
