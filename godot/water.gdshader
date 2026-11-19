shader_type spatial;

// Stylized, lightweight water for the Phase 0 preview.
//
// One transparent plane, one draw call. Vertex waves give a slow swell and the
// fragment stage perturbs the normal analytically for ripples, fresnel, and
// the two-tone shallow/deep colour. No screen-space textures, depth reads,
// reflections, or refraction, so it runs unchanged on gl_compatibility.

render_mode blend_mix, depth_draw_never, cull_disabled;

// Body colour near the surface (lagoons, shallows, sunlit water).
uniform vec4 shallow_color : source_color = vec4(0.24, 0.58, 0.60, 1.0);
// Body colour for deep water.
uniform vec4 deep_color : source_color = vec4(0.05, 0.17, 0.28, 1.0);
// Tint blended in at grazing angles to fake a sky reflection.
uniform vec4 sky_color : source_color = vec4(0.62, 0.80, 0.92, 1.0);
// Surface alpha. 0 is invisible, 1 is fully opaque.
uniform float opacity : hint_range(0.0, 1.0) = 0.74;
// Animation rate multiplier for every wave.
uniform float wave_speed : hint_range(0.0, 4.0) = 0.6;
// Ripple frequency multiplier. Larger is choppier, smaller is glassier.
uniform float wave_scale : hint_range(0.1, 4.0) = 1.0;
// Height of the slow vertex swell, in metres.
uniform float wave_height : hint_range(0.0, 0.5) = 0.05;
// Fresnel falloff. Higher keeps the sky tint closer to the horizon.
uniform float fresnel_power : hint_range(1.0, 8.0) = 4.0;
// Distance at which ripple normals start fading to avoid shimmer.
uniform float detail_distance : hint_range(25.0, 600.0) = 160.0;
// Distance over which the body colour shifts toward the deep tone.
uniform float depth_distance : hint_range(50.0, 2000.0) = 420.0;

varying vec3 world_position;

const float TWO_PI = 6.28318530718;

void vertex() {
	vec3 world = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz;
	float t = TIME * wave_speed;

	// The plane is deliberately coarse (64 m cells at the default 8 km size),
	// so the vertex swell uses wavelengths of hundreds of metres. Fine ripples
	// live in the fragment normal instead of in the mesh.
	float swell = sin(world.x * 0.024 + world.z * 0.011 + t * 0.55)
		+ 0.6 * sin(world.x * -0.009 + world.z * 0.021 + t * 0.80);
	VERTEX.y += swell * wave_height;

	// World-space pattern so the water does not swim when the plane follows
	// the camera.
	world_position = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz;
}

// Analytic derivative of a directional sine wave, returned as a 2D slope.
float wave_slope(vec2 p, vec2 dir, float wavelength, float amplitude, float phase) {
	float k = TWO_PI / max(wavelength, 0.1);
	return amplitude * k * cos(dot(p, dir) * k + phase);
}

void fragment() {
	vec2 p = world_position.xz;
	float t = TIME * wave_speed;
	float camera_distance = distance(CAMERA_POSITION_WORLD, world_position);

	// Fade the normal detail on distant water where one pixel spans many
	// wavelengths. This is the cheapest anti-shimmer measure available without
	// mipmapped procedural noise.
	float detail = 1.0 - smoothstep(detail_distance, detail_distance * 2.5, camera_distance);

	vec2 ripple = vec2(0.0);
	ripple += vec2(0.95, 0.31) * wave_slope(p, normalize(vec2(0.95, 0.31)), 22.0 / wave_scale, 0.120, t * 1.10);
	ripple += vec2(-0.31, 0.95) * wave_slope(p, normalize(vec2(-0.31, 0.95)), 13.0 / wave_scale, 0.065, t * 1.60 + 1.3);
	ripple += vec2(0.70, -0.70) * wave_slope(p, normalize(vec2(0.70, -0.70)), 7.5 / wave_scale, 0.030, t * 2.10 + 2.7);
	ripple *= detail;

	// NORMAL is view space; build the rippled normal in world space first.
	vec3 world_normal = normalize(vec3(-ripple.x, 1.0, -ripple.y));
	vec3 view_normal = normalize((VIEW_MATRIX * vec4(world_normal, 0.0)).xyz);
	NORMAL = view_normal;

	float facing = clamp(dot(view_normal, VIEW), 0.0, 1.0);
	float fresnel = pow(1.0 - facing, fresnel_power);

	// Stylized two-tone body. A slow large-scale patch pattern provides the
	// shallow/deep variation; camera distance is a second depth cue. True
	// bathymetry needs a terrain/depth query and is a later step.
	float patch = 0.5 + 0.5 * sin(p.x * 0.012 + t * 0.07) * sin(p.y * 0.009 - t * 0.05);
	float distance_tint = smoothstep(0.0, depth_distance, camera_distance);
	float deep_mix = clamp(patch * 0.7 + distance_tint * 0.3, 0.0, 1.0);
	vec3 body = mix(shallow_color.rgb, deep_color.rgb, deep_mix);

	ALBEDO = mix(body, sky_color.rgb, fresnel * 0.55);
	ALPHA = clamp(opacity + fresnel * 0.22, 0.0, 1.0);

	// Mostly smooth water with a tiny roughness break from the ripples. The
	// scene's DirectionalLight3D provides the sun glint; no extra light loop.
	ROUGHNESS = mix(0.06, 0.16, clamp(length(ripple) * 2.5, 0.0, 1.0));
	METALLIC = 0.0;
	SPECULAR = 0.6;
}
