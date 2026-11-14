// 视频画面：YUV 4:2:0（亮度一层、色度交错一层，8 位或 16 位无符号整数纹理）→ 屏幕颜色。
// 公式与 color.rs 一一对应，改一边要改另一边。缩小显示时在每个屏幕像素覆盖的源区域里
// 均匀取多点再平均，避免只取一点的闪烁和摩尔纹。

struct Params {
    // R、G、B 三行：[Y 系数, Cb 系数, Cr 系数, 0]。
    row_r: vec4<f32>,
    row_g: vec4<f32>,
    row_b: vec4<f32>,
    // x：码值除数；y：传输（0 普通、1 PQ、2 HLG）；z：BT.2020 → BT.709；
    // w：目标是 sRGB 纹理（要输出线性值，硬件再编码）。
    modes: vec4<f32>,
    // xy：亮度层宽高（纹素）；zw：每轴取几点。
    geometry: vec4<f32>,
    // xy：一个屏幕像素覆盖多少源纹素。
    footprint: vec4<f32>,
};

@group(0) @binding(0) var luma: texture_2d<u32>;
@group(0) @binding(1) var chroma: texture_2d<u32>;
@group(0) @binding(2) var<uniform> params: Params;

const SDR_WHITE_NITS: f32 = 203.0;
const HDR_SOURCE_PEAK_NITS: f32 = 1000.0;
const DISPLAY_GAMMA: f32 = 2.4;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// 一个三角形盖满视口（视口就是画面所在的矩形）。
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var out: VertexOut;
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

fn load_luma(texel: vec2<i32>) -> f32 {
    let size = vec2<i32>(textureDimensions(luma));
    return f32(textureLoad(luma, clamp(texel, vec2<i32>(0), size - 1), 0).r);
}

fn load_chroma(texel: vec2<i32>) -> vec2<f32> {
    let size = vec2<i32>(textureDimensions(chroma));
    return vec2<f32>(textureLoad(chroma, clamp(texel, vec2<i32>(0), size - 1), 0).rg);
}

// 整数纹理不能交给硬件过滤，双线性自己做。position 是纹素坐标（第 i 个纹素中心在 i + 0.5）。
fn bilinear_luma(position: vec2<f32>) -> f32 {
    let p = position - 0.5;
    let base = floor(p);
    let f = p - base;
    let i = vec2<i32>(base);
    let top = mix(load_luma(i), load_luma(i + vec2<i32>(1, 0)), f.x);
    let bottom = mix(load_luma(i + vec2<i32>(0, 1)), load_luma(i + vec2<i32>(1, 1)), f.x);
    return mix(top, bottom, f.y);
}

fn bilinear_chroma(position: vec2<f32>) -> vec2<f32> {
    let p = position - 0.5;
    let base = floor(p);
    let f = p - base;
    let i = vec2<i32>(base);
    let top = mix(load_chroma(i), load_chroma(i + vec2<i32>(1, 0)), f.x);
    let bottom = mix(load_chroma(i + vec2<i32>(0, 1)), load_chroma(i + vec2<i32>(1, 1)), f.x);
    return mix(top, bottom, f.y);
}

fn pq_to_nits(encoded: f32) -> f32 {
    let m1 = 2610.0 / 16384.0;
    let m2 = 2523.0 / 4096.0 * 128.0;
    let c1 = 3424.0 / 4096.0;
    let c2 = 2413.0 / 4096.0 * 32.0;
    let c3 = 2392.0 / 4096.0 * 32.0;
    let power = pow(max(encoded, 0.0), 1.0 / m2);
    return 10000.0 * pow(max(power - c1, 0.0) / (c2 - c3 * power), 1.0 / m1);
}

fn nits_to_pq(nits: f32) -> f32 {
    let m1 = 2610.0 / 16384.0;
    let m2 = 2523.0 / 4096.0 * 128.0;
    let c1 = 3424.0 / 4096.0;
    let c2 = 2413.0 / 4096.0 * 32.0;
    let c3 = 2392.0 / 4096.0 * 32.0;
    let power = pow(max(nits / 10000.0, 0.0), m1);
    return pow((c1 + c2 * power) / (1.0 + c3 * power), m2);
}

fn hlg_to_scene(encoded: f32) -> f32 {
    let a = 0.17883277;
    let b = 0.28466892;
    let c = 0.5599107;
    if encoded <= 0.5 {
        return encoded * encoded / 3.0;
    }
    return (exp((encoded - c) / a) + b) / 12.0;
}

fn hlg_nits(encoded: vec3<f32>) -> vec3<f32> {
    let scene = vec3<f32>(hlg_to_scene(encoded.r), hlg_to_scene(encoded.g), hlg_to_scene(encoded.b));
    let luminance = dot(scene, vec3<f32>(0.2627, 0.6780, 0.0593));
    return scene * (HDR_SOURCE_PEAK_NITS * pow(max(luminance, 0.0), 0.2));
}

fn gamut(rgb: vec3<f32>) -> vec3<f32> {
    if params.modes.z < 0.5 {
        return rgb;
    }
    return vec3<f32>(
        dot(rgb, vec3<f32>(1.6605, -0.5876, -0.0728)),
        dot(rgb, vec3<f32>(-0.1246, 1.1329, -0.0083)),
        dot(rgb, vec3<f32>(-0.0182, -0.1006, 1.1187)),
    );
}

fn compressed_nits(nits: f32) -> f32 {
    let source_peak = nits_to_pq(HDR_SOURCE_PEAK_NITS);
    let normalized = min(nits_to_pq(nits) / source_peak, 1.0);
    let white = nits_to_pq(SDR_WHITE_NITS) / source_peak;
    let knee = 1.5 * white - 0.5;
    var compressed = normalized;
    if normalized >= knee {
        let t = (normalized - knee) / (1.0 - knee);
        let t2 = t * t;
        let t3 = t2 * t;
        compressed = (2.0 * t3 - 3.0 * t2 + 1.0) * knee
            + (t3 - 2.0 * t2 + t) * (1.0 - knee)
            + (-2.0 * t3 + 3.0 * t2) * white;
    }
    return pq_to_nits(compressed * source_peak);
}

fn tone_map(nits: vec3<f32>) -> vec3<f32> {
    let peak = max(nits.r, max(nits.g, nits.b));
    if peak <= 0.0 {
        return vec3<f32>(0.0);
    }
    return max(nits, vec3<f32>(0.0)) * (compressed_nits(peak) / peak / SDR_WHITE_NITS);
}

// 和 ColorParams::display_rgb 一样：码值 → 按显示伽马编码的 RGB。
fn display_rgb(codes: vec3<f32>) -> vec3<f32> {
    let divisor = params.modes.x;
    let ycc = vec3<f32>(
        (codes.x / divisor - 16.0) / 219.0,
        (codes.y / divisor - 128.0) / 224.0,
        (codes.z / divisor - 128.0) / 224.0,
    );
    let encoded = clamp(
        vec3<f32>(dot(params.row_r.xyz, ycc), dot(params.row_g.xyz, ycc), dot(params.row_b.xyz, ycc)),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    var light: vec3<f32>;
    if params.modes.y > 1.5 {
        light = tone_map(gamut(hlg_nits(encoded)));
    } else if params.modes.y > 0.5 {
        light = tone_map(gamut(vec3<f32>(pq_to_nits(encoded.r), pq_to_nits(encoded.g), pq_to_nits(encoded.b))));
    } else if params.modes.z > 0.5 {
        light = gamut(pow(encoded, vec3<f32>(DISPLAY_GAMMA)));
    } else {
        return encoded;
    }
    return pow(clamp(light, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / DISPLAY_GAMMA));
}

// 目标是 sRGB 纹理时硬件会再编码一次：先按 sRGB 解码，屏幕上出来的就是上面算的值。
fn srgb_to_linear(encoded: vec3<f32>) -> vec3<f32> {
    let low = encoded / 12.92;
    let high = pow((encoded + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, encoded <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let center = in.uv * params.geometry.xy;
    let taps = vec2<i32>(params.geometry.zw);
    var sum = vec3<f32>(0.0);
    for (var ty = 0; ty < taps.y; ty++) {
        for (var tx = 0; tx < taps.x; tx++) {
            let offset = (vec2<f32>(f32(tx), f32(ty)) + 0.5) / vec2<f32>(taps) - 0.5;
            let position = center + offset * params.footprint.xy;
            let c = bilinear_chroma(position * 0.5);
            sum += vec3<f32>(bilinear_luma(position), c.x, c.y);
        }
    }
    let rgb = display_rgb(sum / f32(taps.x * taps.y));
    if params.modes.w > 0.5 {
        return vec4<f32>(srgb_to_linear(rgb), 1.0);
    }
    return vec4<f32>(rgb, 1.0);
}
