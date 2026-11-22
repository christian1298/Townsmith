extends RefCounted

## Shared benchmark camera pose helper.
##
## Fixed benchmark routes interpolate a position and a look-at target. Two
## shapes break `Camera3D.look_at`:
##   - `pos == look`, where the forward axis is zero and `look_at` pushes an
##     error every frame, and
##   - a target directly above or below the camera, where the default
##     `Vector3.UP` up vector is collinear with the forward axis.
## The default `phase0_flyover.json` route contains the second shape: its
## `[128, 60, 128] -> [128, 30, 128]` waypoint is exactly vertical.
##
## A degenerate (`pos == look`) sample is skipped after a one-time warning; the
## camera keeps its previous pose, so a malformed route degrades to a frozen
## frame instead of an error flood. A near-vertical sample swaps the up vector
## to `Vector3.FORWARD`, the same fallback `mayor_camera.gd` uses when its pitch
## passes 89 degrees.

## Below this separation (metres) the sample has no usable forward axis.
const MIN_FORWARD_M := 0.0001
## Absolute dot of the normalized forward axis with `Vector3.UP` above which
## the up vector is swapped for `Vector3.FORWARD` (the 89 degree pitch cut).
const VERTICAL_DOT := 0.999

static var _warned_degenerate := false

## Places `camera` at `pos`, looking at `look`. Returns false when the sample
## was skipped as degenerate.
static func apply_pose(camera: Camera3D, pos: Vector3, look: Vector3) -> bool:
	if camera == null:
		return false
	var forward := look - pos
	if forward.length() < MIN_FORWARD_M:
		if not _warned_degenerate:
			_warned_degenerate = true
			push_warning(
				"bench_route: skipping coincident pose (position == look_at %s); later samples stay silent"
				% str(pos)
			)
		return false
	camera.position = pos
	var up := Vector3.UP
	if absf(forward.normalized().dot(Vector3.UP)) > VERTICAL_DOT:
		up = Vector3.FORWARD
	camera.look_at(look, up)
	return true
