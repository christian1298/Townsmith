---
title: Water
project: Townsmith
status: draft
updated: 2026-11-19
tags:
  - townsmith
  - rendering
  - shader
  - phase-0
  - water
---

# 20 — Water

> [!info] What this note is
> The Phase 0 water preview. It is a stylized, lightweight shader for the Godot shell, not the final water simulation. See [[13 - Art Direction]] for the visual target and [[18 - Phase 0 Implementation]] for the current build.

## 1. Goals

- Read as water at a glance from the Mayor camera and at street level.
- Match the blocky, limited-palette look; no photoreal reflections or refraction.
- One transparent draw call; no screen-space effects.
- Work on `gl_compatibility`, not only Forward+.
- Fade ripple detail with distance to avoid shimmer at city scale.

## 2. Approach

`godot/water.gdshader` is a `spatial` shader with:

- Two long vertex swells. The plane is coarse (64 m cells at 8 km), so fine waves are not vertex geometry.
- Three analytic directional ripples in the fragment shader. They perturb the world-space normal and fade out with camera distance.
- A two-tone body: `shallow_color` to `deep_color` using a slow large-scale patch plus camera-distance tint.
- A fresnel blend toward `sky_color` at grazing angles.
- Built-in `DirectionalLight3D` specular through `ROUGHNESS` / `SPECULAR`; no custom light loop.
- `blend_mix`, `depth_draw_never`, `cull_disabled`.

`godot/water_body.gd` owns one `MeshInstance3D` with an 8192 m `PlaneMesh` (128 × 128 subdivisions, ~33k triangles), a `ShaderMaterial`, and a `follow(target)` method that centers the plane on the camera in XZ.

`godot/terrain_streamer.gd` creates the water body, sets it to `WATER_HEIGHT` (24.1 m, 0.1 m above the Rust sea level of 24 m), and calls `follow` whenever the camera moves.

## 3. Uniforms

| Uniform | Default | Purpose |
|---|---:|---|
| `shallow_color` | `#3d9499` | Shallow body colour |
| `deep_color` | `#0d2b47` | Deep body colour |
| `sky_color` | `#9ecceb` | Fresnel sky tint |
| `opacity` | `0.74` | Base surface alpha |
| `wave_speed` | `0.6` | Animation rate multiplier |
| `wave_scale` | `1.0` | Ripple frequency multiplier |
| `wave_height` | `0.05` | Vertex swell height in metres |
| `fresnel_power` | `4.0` | Fresnel falloff |
| `detail_distance` | `160` | Ripple fade start distance |
| `depth_distance` | `420` | Deep-tint distance |

## 4. Performance

- One water draw call.
- ~33k triangles for the plane at the default 8 km size and 128 subdivisions.
- Three trigonometric ripple evaluations per water fragment, plus one fresnel term.
- No depth texture reads, screen-space refraction, reflection probes, or foam textures.
- Shadow casting is disabled on the water body.

Measured on the Phase 0 streaming route in headless mode is not representative of GPU cost. A display run on the target machine is still required.

## 5. Limitations

- Water is one global height; there is no per-body lake/river surface yet.
- No depth-based shoreline, foam, caustics, flow maps, reflections, or refraction.
- The two-tone colour is stylized, not bathymetry.
- `sky_color` is a manual match to the current sky; a later environment system should drive it.
- Water does not affect gameplay (fishing, flooding, transport) yet.
- There is no underwater rendering or camera transition.

## 6. Next steps

1. Visual check on a display and on the target GPU.
2. Depth-based shoreline/foam from the terrain heightmap.
3. Separate water bodies with their own levels (lakes, rivers, sea).
4. Environment-driven sky tint and sun direction.
5. Gameplay hooks: fishing, flooding, boats, water supply.
