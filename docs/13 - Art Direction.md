---
title: Art Direction
project: Townsmith
status: draft
updated: 2026-11-19
tags:
  - townsmith
  - art
  - audio
---

# 13 — Art Direction

## 1. Visual identity

**Minecraft-inspired, Teardown-detailed.**

- **Chunky, readable silhouettes.** Buildings should be identifiable from the mayor's altitude by shape and roof line.
- **Material honesty.** Wood looks like wood, brick like brick, steel like steel. The player reads the construction to understand the building.
- **Warm, lived-in, optimistic.** Sunny streets, warm windows, gardens, smoke from chimneys. Problems are visible but solvable.
- **Not photoreal.** No ray-traced reflections, no film grain, no gritty desaturation. The look is stylized and clean.
- **Scale contrast is intentional.** 1 m terrain blocks give the Minecraft landscape; 10 cm building voxels give the first-person detail.

## 2. The 10 cm texture problem

At 10 cm, a 1 m wall is **10 voxels across**. If each voxel face uses a 16×16 texture, that is **160 texels per meter** — denser than many realistic games. The result can look noisy or "static" rather than Minecraft-like.

### Guidelines

| Surface | Treatment |
|---|---|
| Large walls | 16×16 texture, low contrast, mipmaps; pattern repeats at 1 m or 2 m |
| Floors | 16×16 with subtle variation; avoid high-frequency noise |
| Roofs | Larger pattern (tiles, shingles) across multiple voxels; use material color variation |
| Trim, doors, windows | Higher contrast; these carry readability |
| Foliage | Alpha-tested cards or chunky voxel leaves; no photoreal foliage |
| Glass | Separate transparent pass; keep it simple and readable |
| Metal | Flat color + roughness variation; no realistic reflections |

- **Limited palette:** 8–16 colors per material family. The Minecraft look comes from palette discipline, not texture resolution.
- **Mipmaps and anisotropic filtering** are mandatory; without them, distant buildings shimmer.
- **Per-voxel color variation** (subtle) can break repetition without a texture.
- **Resource packs** can replace textures; the base game must look good with flat colors alone.

> [!question] Q-026 — Texture resolution
> 16×16 per face is the default. 8×8 is a low-end/retro option; 32×32 is a resource-pack option. Test all three in the Phase 1 art pass.

## 3. Materials and palette

### Material families

| Family | Examples | Visual cues |
|---|---|---|
| Timber | Log, plank, beam, thatch | Warm browns, visible grain, irregular |
| Masonry | Stone, brick, plaster, tile | Cool grays, reds, mortar lines |
| Concrete | Poured concrete, reinforced, precast | Flat gray, seams, stains |
| Metal | Iron, steel, copper, aluminum | Cool grays, patina, rivets |
| Glass | Window, greenhouse, curtain wall | Transparency, reflections (simple) |
| Textiles | Carpet, curtain, awning | Saturated colors, soft patterns |
| Foliage | Leaves, grass, crops | Green/yellow variation, alpha |
| Water | River, canal, fountain | Animated normal/scroll, simple reflections |

### Palette by age

- **Wood:** warm browns, thatch yellow, clay red, off-white plaster.
- **Masonry:** brick red, limestone, slate, deep green accents.
- **Industrial:** soot black, rust, steel blue, brick, amber windows.
- **Modern:** concrete gray, glass blue-green, white, bold accents.
- **Information:** clean whites, solar blue, green roofs, warm LEDs.

- The palette should shift gradually; a city should look like it has layers of history.
- Avoid a single global color filter. Age is expressed through materials, not grading.

## 4. Lighting

- **Sun + sky:** a simple directional sun, sky ambient, and a soft shadow map.
- **Ambient occlusion:** per-vertex AO in the voxel mesher is the most important visual cue. Without it, voxel buildings look flat.
- **Interior light:** emissive windows and lamps; no dynamic voxel light propagation in v1.
- **Night:** warm windows, streetlights, cool moonlight; avoid pitch black.
- **Weather:** clear, overcast, rain, snow (later); simple particle effects.
- **Post-processing:** minimal. Maybe SSAO, color grading, and a subtle vignette. No heavy bloom or lens flares.

> [!question] Q-027 — Voxel lighting
> Dynamic torch/lamp propagation is a post-v1 feature. v1 uses emissive materials and baked/vertex lighting.

## 5. World and living world art

The open world is a major part of the visual identity. It should feel like a place, not a backdrop.

### Biomes

| Biome | Palette | Landmarks |
|---|---|---|
| **Forest** | Deep greens, brown bark, moss | Dense trees, clearings, logging camps |
| **Plains** | Warm grass, wheat, wildflowers | Farms, windbreaks, distant hills |
| **Hills** | Rock, scrub, terraced slopes | Mines, switchback roads, lodges |
| **Coast** | Sand, blue-green water, weathered wood | Fishing huts, docks, salt air |
| **River valley** | Lush green, reeds, stone | Bridges, mills, irrigation |

- Transitions should be gradual; biome edges are blended with terrain and vegetation, not hard seams.
- Each biome needs a distinct silhouette at a glance, not just a color swap.

### Trees, crops, and animals

- **Trees:** voxel-generated or authored props; species must be readable by silhouette (oak, pine, birch, fruit tree). Use LOD/impostors beyond ~100 m; instanced near the camera.
- **Crops:** field plots with visible growth stages; use instanced meshes or a shader-driven field, not individual plants. Seasonal colors: green → gold → stubble → snow.
- **Livestock/wildlife:** stylized low-poly models with readable silhouettes; not voxel-built (too heavy for animation). Deer, boar, rabbits, wolves, cows, chickens, sheep.
- **Hunting:** avoid gore; use stylized reactions, tracks, and carry poses.
- **Logs and goods:** visible stacks, carts, and carrying animations make the primary industries legible.

### Early settlement kit

- **Lumber camp:** log pile, sawbuck, stump field, saplings in neat rows.
- **Farm:** barn, silo, scarecrow, fences, crop rows, a cart.
- **Hunting lodge:** antlers, pelts, drying rack, a trail into the woods.
- **Well and market:** the first civic buildings; they should feel like a village, not a city.

### Seasons

- **Spring:** blossoms, muddy roads, planting.
- **Summer:** full green, long light, harvest prep.
- **Autumn:** orange/red leaves, golden fields, harvest.
- **Winter:** snow cover, bare trees, smoke, short days.
- Seasonal palette shifts should be gradual and readable; snow should not hide the voxel detail entirely.
- Resource packs can override seasonal textures; the base game must still read.

### Performance rules

- Instanced trees/crops near the camera; impostors beyond.
- No individual grass blades; use terrain textures and clumps.
- Wildlife and livestock are low-poly with shared skeletons.
- Seasonal changes are material/texture swaps, not new meshes.

## 6. Readability at distance

- **Silhouette first:** roof shape, chimneys, towers, signage.
- **Color second:** material family and age.
- **Detail last:** windows, trim, props.
- **LOD must preserve silhouette and dominant color.** A distant bakery should still read as a bakery.
- **Impostors:** baked billboards with alpha; fade between LODs to hide popping.
- **Avoid visual clutter:** too many props at city scale becomes noise. Use them where the player looks.

## 7. UI and UX art

- **City-builder dashboard:** clear hierarchy, readable numbers, trends.
- **Dark and light themes.** Default to dark for long sessions; light for accessibility.
- **Colorblind-safe overlays:** use patterns, icons, and labels, not just red/green.
- **Scalable text:** 100–200% UI scale; no fixed pixel fonts.
- **Icons:** simple, consistent, 2 px strokes; material icons match the palette.
- **Blueprint cards:** thumbnail, name, age, capacity, cost, class, tags.
- **Build tool:** ghost preview, green/red validity, cost tooltip, rotation gizmo.
- **Diegetic elements:** street signs, shop signs, district names; keep them optional.
- **No UI noise:** hide advanced stats behind a tooltip or an "info" toggle.

## 8. Audio

| Layer | Examples |
|---|---|
| **Ambience** | Wind, birds, distant traffic, river, construction |
| **City** | Crowds, shops, trains, cars, machinery |
| **Building** | Door creaks, ovens, looms, generators |
| **UI** | Clicks, placement, errors, notifications |
| **Music** | Calm, age-based layers; grows with the city |

- **Dynamic mix:** the camera altitude changes the mix (mayor view = broader city ambience; street view = detail).
- **Age-based music:** wood age = acoustic; industrial = rhythmic; modern = electronic; information = ambient.
- **No licensed music** at launch; original or licensed tracks later.
- **Accessibility:** separate music/SFX/ambience sliders; mono option; subtitles for notifications.
- **Performance:** stream long ambience; limit concurrent 3D sources; use distance attenuation.

## 9. Resource pack implications

- Textures are addressed by **material ID**, not file path.
- Resource packs can replace: material textures, prop textures/models, UI theme, icons, fonts, sounds, music.
- **No gameplay changes.** A resource pack cannot alter stats, recipes, or building rules.
- Base game must be fully playable with **flat colors** if textures fail to load.
- Provide a **template pack** with the correct folder structure and a manifest.
- Document texture size, alpha rules, mipmaps, and color space.
- Shader overrides are allowed with a compatibility warning (Q-038).

## 10. Technical constraints

- **Texture arrays**, not atlases, for materials.
- **Mipmaps + anisotropic filtering** for all voxel textures.
- **Alpha-tested** foliage; **alpha-blended** glass in a separate pass.
- **No per-voxel dynamic lights** in v1.
- **LOD/impostors** must be authored or generated consistently.
- **Memory budget:** texture arrays are the dominant VRAM cost; keep base materials at 16×16 and allow 32×32 resource packs.
- **Shader complexity:** keep the voxel shader cheap; the city has many chunks and instances.
- **Color space:** sRGB textures; linear lighting; consistent palette across ages.

## 11. Inspirations

| Source | What to study |
|---|---|
| **Minecraft** | Palette discipline, readability, resource packs, modding |
| **Teardown** | 10 cm detail, first-person scale, lighting |
| **Cities: Skylines II** | City readability, overlays, UI density |
| **Timberborn** | Wood-era charm, modular building |
| **Anno 1800** | Age progression and city beauty |
| **Foundation** | Organic medieval growth, readable silhouettes |
| **The Sims** | Interior readability, furniture semantics |

## 12. Open art questions

- **Q-008:** Visual target and resource pack importance.
- **Q-026:** Texture resolution per voxel face.
- **Q-027:** Voxel lighting approach.
- **Q-038:** Shader support in resource packs.
- **Q-059:** Art style for citizens: stylized low-poly, voxel, or billboard?
- **Q-060:** How much visual variation between biomes/maps at launch? Current target: 3–4 biomes.
- **Q-061:** UI language: clean modern dashboard, or diegetic/newspaper style?
- **Q-065:** How deep is the living world visually (trees, crops, animals)?
- **Q-069:** Seasons at launch? Art needs spring/summer/autumn/winter palettes if yes.

See [[09 - Open Questions]].
