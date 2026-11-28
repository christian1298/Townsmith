extends RefCounted

## Master-volume settings for the front-end shell (docs/86).
##
## Persistence is a small `ConfigFile` at `user://settings.cfg`:
##
##     [settings]
##     version = 1
##
##     [audio]
##     master_volume = 100
##     muted = false
##
## `version` lets a later shell migrate the file instead of guessing. A
## missing, unreadable, or future-version file falls back to the defaults
## (100 / unmuted) and never aborts: the menu is the first scene, so a broken
## settings file must not be able to block startup.
##
## Volume is a whole number 0..100. Out-of-range numbers clamp; values of the
## wrong type fall back to the default instead of being coerced. The engine
## value is linear, the bus value is dB, and mute is a separate bus flag, so a
## muted game still remembers the level it will restore.
##
## The apply step is the only AudioServer dependency; the rest is pure and is
## exercised directly by `--menu-smoke`.

const SETTINGS_PATH := "user://settings.cfg"
const VERSION := 1
const DEFAULT_VOLUME := 100
const DEFAULT_MUTED := false
const MIN_VOLUME := 0
const MAX_VOLUME := 100
## linear_to_db(0) is -inf; keep the floor at -80 dB so the bus value stays a
## finite number and the smokes can compare it.
const MIN_LINEAR := 0.0001


## Whole-number volume in 0..100. Numbers round and clamp; anything else is
## the default. Booleans are deliberately not numbers here.
static func sanitize_volume(raw) -> int:
	if typeof(raw) != TYPE_INT and typeof(raw) != TYPE_FLOAT:
		return DEFAULT_VOLUME
	if typeof(raw) == TYPE_FLOAT and not is_finite(raw):
		return DEFAULT_VOLUME
	return clampi(roundi(float(raw)), MIN_VOLUME, MAX_VOLUME)


static func sanitize_muted(raw) -> bool:
	if typeof(raw) == TYPE_BOOL:
		return raw
	return DEFAULT_MUTED


## Defaults, also used as the fallback for every rejected file.
static func defaults() -> Dictionary:
	return {
		"version": VERSION,
		"volume": DEFAULT_VOLUME,
		"muted": DEFAULT_MUTED,
		"loaded": false,
	}


## Read `path` and return `{version, volume, muted, loaded}`. `loaded` is true
## only when the file was read and accepted, so callers can tell a defaults
## fallback from a real save.
static func load_from(path: String = SETTINGS_PATH) -> Dictionary:
	var result := defaults()
	if path.is_empty() or not FileAccess.file_exists(path):
		return result
	var config := ConfigFile.new()
	if config.load(path) != OK:
		return result
	var version := int(config.get_value("settings", "version", VERSION))
	if version < 1 or version > VERSION:
		return result
	result["version"] = version
	result["volume"] = sanitize_volume(
		config.get_value("audio", "master_volume", DEFAULT_VOLUME)
	)
	result["muted"] = sanitize_muted(config.get_value("audio", "muted", DEFAULT_MUTED))
	result["loaded"] = true
	return result


## Write `version`, `master_volume`, and `muted`. Returns the `ConfigFile` error
## so the caller can keep the overlay open instead of pretending the save worked.
static func save_to(path: String, volume: int, muted: bool) -> Error:
	if path.is_empty():
		return ERR_INVALID_PARAMETER
	var dir := path.get_base_dir()
	if not dir.is_empty() and not DirAccess.dir_exists_absolute(dir):
		var dir_error := DirAccess.make_dir_recursive_absolute(dir)
		if dir_error != OK:
			return dir_error
	var config := ConfigFile.new()
	config.set_value("settings", "version", VERSION)
	config.set_value("audio", "master_volume", sanitize_volume(volume))
	config.set_value("audio", "muted", sanitize_muted(muted))
	return config.save(path)


## Push volume/mute to the main bus. Bus 0 is the engine's master bus; the
## headless dummy driver still answers these calls, and the guards keep a
## stripped bus layout from crashing the menu.
static func apply(volume: int, muted: bool) -> void:
	if AudioServer.get_bus_count() == 0:
		return
	var linear := clampf(float(sanitize_volume(volume)) / 100.0, MIN_LINEAR, 1.0)
	AudioServer.set_bus_volume_db(0, linear_to_db(linear))
	AudioServer.set_bus_mute(0, sanitize_muted(muted))
