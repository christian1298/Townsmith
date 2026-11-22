extends Control

## Flat top-down preview of one server `interest_view` road set.
##
## The panel is deliberately bounded: at most [`InterestRender.MAX_ROADS`] line
## segments, one per returned road, fitted around the viewport centre/radius.
## It is a protocol-visibility aid for the loopback smoke, not the world
## renderer: it reads no terrain, chunks, citizens, or production city data, and
## it does not stream or mutate anything. `clear_view()` on disconnect removes
## every segment (and resets the reported line count to zero).
##
## See `godot/mp_interest_render.gd` for the pure geometry/checksum helpers and
## docs/05 - Multiplayer.md §Protocol v0.

const InterestRender = preload("res://mp_interest_render.gd")

const PANEL_BACKGROUND := Color(0.030, 0.038, 0.050, 1.0)
const PANEL_BORDER := Color(0.20, 0.24, 0.30, 1.0)
const VIEWPORT_FILL := Color(0.075, 0.105, 0.145, 0.72)
const VIEWPORT_RING := Color(0.32, 0.44, 0.58, 1.0)
const GRID_COLOR := Color(0.16, 0.20, 0.26, 0.75)
const ROAD_OUTLINE := Color(0.035, 0.040, 0.048, 1.0)
const ROAD_COLOR := Color(0.96, 0.78, 0.32, 1.0)
const ROAD_ENDPOINT := Color(1.0, 0.92, 0.62, 1.0)
const TRUNCATED_COLOR := Color(0.92, 0.42, 0.30, 1.0)
const MARGIN_PX := 12.0

var center := Vector2.ZERO
var radius_m := 0.0
var roads: Array = []
var truncated := false
var has_view := false

func _ready() -> void:
	clip_contents = true
	mouse_filter = Control.MOUSE_FILTER_IGNORE
	custom_minimum_size = Vector2(560.0, 240.0)

## Store a view and return the number of road lines it can draw.
func set_view(
	view_center: Vector2, view_radius_m: float, raw_roads: Variant, is_truncated: bool
) -> int:
	center = view_center
	radius_m = maxf(view_radius_m, 0.0)
	roads = InterestRender.parse_roads(raw_roads)
	truncated = is_truncated
	has_view = true
	queue_redraw()
	return roads.size()

## Remove the previous view. Called on disconnect and before a new interest.
func clear_view() -> void:
	center = Vector2.ZERO
	radius_m = 0.0
	roads.clear()
	truncated = false
	has_view = false
	queue_redraw()

## Number of rendered road lines for the current view. Zero after clear.
func line_count() -> int:
	return roads.size() if has_view else 0

## Deterministic 16-hex checksum over the bounded road ids/endpoints.
func road_checksum() -> String:
	if not has_view:
		return ""
	return InterestRender.checksum(roads)

## One-line label value for the client UI.
func summary() -> String:
	if not has_view:
		return "roads=0 truncated=false"
	return "roads=%d truncated=%s center=(%.1f, %.1f) radius=%.1fm" % [
		roads.size(),
		str(truncated),
		center.x,
		center.y,
		radius_m,
	]

func _draw() -> void:
	var bounds := Rect2(Vector2.ZERO, size)
	draw_rect(bounds, PANEL_BACKGROUND, true)
	draw_rect(bounds, PANEL_BORDER, false, 1.0)
	if not has_view:
		return

	var scale := InterestRender.pixels_per_metre(radius_m, size, MARGIN_PX)
	var center_px := InterestRender.project_point(center, center, radius_m, size, MARGIN_PX)
	var radius_px := InterestRender.circle_radius_px(radius_m, size, MARGIN_PX)

	draw_circle(center_px, radius_px, VIEWPORT_FILL)
	draw_arc(center_px, radius_px, 0.0, TAU, 96, VIEWPORT_RING, 1.0, true)
	draw_line(
		Vector2(center_px.x, MARGIN_PX),
		Vector2(center_px.x, size.y - MARGIN_PX),
		GRID_COLOR,
		1.0,
		true
	)
	draw_line(
		Vector2(MARGIN_PX, center_px.y),
		Vector2(size.x - MARGIN_PX, center_px.y),
		GRID_COLOR,
		1.0,
		true
	)

	var road_segments := InterestRender.segments(roads, center, radius_m, size, MARGIN_PX)
	for road_segment in road_segments:
		var start: Vector2 = road_segment["start"]
		var end: Vector2 = road_segment["end"]
		var width_px := float(road_segment["width_px"])
		draw_line(start, end, ROAD_OUTLINE, width_px + 2.0, true)
		draw_line(start, end, ROAD_COLOR, width_px, true)
		draw_circle(start, maxf(2.0, width_px * 0.30), ROAD_ENDPOINT)
		draw_circle(end, maxf(2.0, width_px * 0.30), ROAD_ENDPOINT)

	if scale > 0.0 and truncated:
		draw_rect(bounds, TRUNCATED_COLOR, false, 2.0)
