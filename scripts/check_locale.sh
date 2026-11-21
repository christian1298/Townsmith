#!/usr/bin/env bash
# Static EN/DE locale table gate for the Godot slices.
#
# Parses the runtime `TABLES` const in `godot/editor_i18n.gd` and
# `godot/world_i18n.gd`, plus the `DISPLAY` const in
# `godot/content_i18n.gd`, without Godot, Python, or any third-party tool and
# asserts:
#   1. EN and DE expose the same key set (no duplicates either).
#   2. Each locale meets its file's key floor (ratchet floor).
#   3. No value is empty or whitespace-only.
#   4. `{0}` / `{name}` placeholder sets match between EN and DE per key.
#
# Usage:
#   scripts/check_locale.sh            # both tables, independent floors
#   scripts/check_locale.sh [FILE]     # one table; world_i18n.gd gets world floor
#   scripts/check_locale.sh --self-test
#
# Default success prints one line per table plus a summary:
#   LOCALE_CHECK file=godot/editor_i18n.gd keys=96 min=96 status=ok
#   LOCALE_CHECK file=godot/world_i18n.gd keys=33 min=33 status=ok
#   LOCALE_CHECK file=godot/content_i18n.gd keys=32 min=32 status=ok
#   LOCALE_CHECK editor=96 world=33 content=32 status=ok
#
# Single-file success keeps the original contract:
#   LOCALE_CHECK keys=96 status=ok
#
# Failure prints `LOCALE_CHECK status=fail` plus file/key details and exits 1.
# `--self-test` builds throwaway fixtures and proves the failure paths fire.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/scripts/$(basename "${BASH_SOURCE[0]}")"
EDITOR_FILE="$ROOT/godot/editor_i18n.gd"
WORLD_FILE="$ROOT/godot/world_i18n.gd"
CONTENT_FILE="$ROOT/godot/content_i18n.gd"
EDITOR_MIN_KEYS=96
WORLD_MIN_KEYS=33
CONTENT_MIN_KEYS=32
LOCALES=(en de)

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

usage() {
	echo "usage: $(basename "$0") [FILE] | --self-test" >&2
	exit 2
}

# Print `key<TAB>value` for one locale block of the TABLES const.
parse_locale() {
	local locale="$1" file="$2" marker="$3"
	awk -v locale="$locale" -v marker="$marker" '
		function bad_line(message) {
			printf "locale parse error: %s (line %d: %s)\n", message, NR, $0 > "/dev/stderr"
			bad = 1
		}
		$0 == "const " marker " := {" { in_table = 1; next }
		!in_table { next }
		$0 == "\t\"" locale "\": {" { in_locale = 1; next }
		in_locale && $0 == "\t}," { exit }
		in_locale && $0 ~ /^[ \t]*#/ { next }
		in_locale && $0 ~ /^[ \t]*$/ { next }
		in_locale && substr($0, 1, 3) == "\t\t\"" {
			rest = substr($0, 4)
			pos = index(rest, "\": \"")
			if (pos == 0) {
				bad_line("missing key/value separator")
				next
			}
			key = substr(rest, 1, pos - 1)
			value = substr(rest, pos + 4)
			if (value ~ /",$/) {
				value = substr(value, 1, length(value) - 2)
			} else {
				sub(/"$/, "", value)
			}
			printf "%s\t%s\n", key, value
			next
		}
		in_locale {
			bad_line("unexpected line in locale block")
		}
		END { if (bad) exit 3 }
	' "$file"
}

add_issue() {
	printf '%s\n' "$*" >> "$TMP/issues"
}

# Sorted keys present in `reference` but absent from `target`.
missing_keys() {
	LC_ALL=C comm -23 "$TMP/$1.keys" "$TMP/$2.keys"
}

# Marker const for a file.
marker_for_file() {
	local file="$1"
	if [[ "$(basename "$file")" == "content_i18n.gd" ]]; then
		printf '%s\n' "DISPLAY"
	else
		printf '%s\n' "TABLES"
	fi
}

# Floor for a file: world and content tables ratchet independently.
floor_for_file() {
	local file="$1"
	case "$(basename "$file")" in
		world_i18n.gd) printf '%s\n' "$WORLD_MIN_KEYS" ;;
		content_i18n.gd) printf '%s\n' "$CONTENT_MIN_KEYS" ;;
		*) printf '%s\n' "$EDITOR_MIN_KEYS" ;;
	esac
}

# Validate one file. Issues land in "$TMP/issues"; the EN key count is left in
# AUDIT_COUNT (0 when the table could not be read).
audit_file() {
	local file="$1" min="$2" locale keys_file count duplicates marker
	: > "$TMP/issues"
	AUDIT_COUNT=0
	marker="$(marker_for_file "$file")"

	if [[ ! -f "$file" ]]; then
		add_issue "file not found: $file"
	else
		for locale in "${LOCALES[@]}"; do
			if ! parse_locale "$locale" "$file" "$marker" > "$TMP/$locale.tsv"; then
				add_issue "could not parse the '$locale' table in $file"
			fi
			keys_file="$TMP/$locale.keys"
			cut -f1 "$TMP/$locale.tsv" | LC_ALL=C sort > "$keys_file"

			count="$(wc -l < "$keys_file" | tr -d ' ')"
			if (( count < min )); then
				add_issue "$locale has $count keys, minimum is $min"
			fi

			duplicates="$(uniq -d "$keys_file" | head -n 5)"
			if [[ -n "$duplicates" ]]; then
				while IFS= read -r key; do
					add_issue "duplicate key in $locale: $key"
				done <<< "$duplicates"
			fi
		done
		AUDIT_COUNT="$(wc -l < "$TMP/en.keys" | tr -d ' ')"
	fi

	if [[ -s "$TMP/en.tsv" && -s "$TMP/de.tsv" ]]; then
		local missing_de missing_en
		missing_de="$(missing_keys en de | head -n 20)"
		missing_en="$(missing_keys de en | head -n 20)"
		if [[ -n "$missing_de" || -n "$missing_en" ]]; then
			add_issue "EN and DE key sets differ"
			while IFS= read -r key; do
				[[ -n "$key" ]] && add_issue "  missing in DE: $key"
			done <<< "$missing_de"
			while IFS= read -r key; do
				[[ -n "$key" ]] && add_issue "  missing in EN: $key"
			done <<< "$missing_en"
		fi

		for locale in "${LOCALES[@]}"; do
			awk -F '\t' -v locale="$locale" '
				{
					value = $2
					gsub(/^[ \t]+|[ \t]+$/, "", value)
					if (value == "") {
						printf "empty value: %s %s\n", locale, $1
					}
				}
			' "$TMP/$locale.tsv" >> "$TMP/issues"
		done

		# Compare placeholder sets, not values: `{0}` and `{name}` may move
		# inside a translation but the parameter contract must stay identical.
		awk -F '\t' '
			function collect(text, found,    rest, key) {
				delete found
				n = 0
				rest = text
				while (match(rest, /\{[A-Za-z0-9_]+\}/)) {
					key = substr(rest, RSTART, RLENGTH)
					rest = substr(rest, RSTART + RLENGTH)
					if (!(key in found)) {
						found[key] = 1
						n++
					}
				}
				return n
			}
			function join_set(found,    name, out, first) {
				out = ""
				first = 1
				for (name in found) {
					if (!first) {
						out = out ","
					}
					out = out name
					first = 0
				}
				return out
			}
			FNR == NR { en[$1] = $2; next }
			{
				if (!($1 in en)) {
					next
				}
				# Empty values are reported above; do not double-report them.
				if (en[$1] ~ /^[ \t]*$/ || $2 ~ /^[ \t]*$/) {
					next
				}
				en_count = collect(en[$1], en_placeholders)
				de_count = collect($2, de_placeholders)
				mismatch = (en_count != de_count)
				if (!mismatch) {
					for (name in en_placeholders) {
						if (!(name in de_placeholders)) {
							mismatch = 1
							break
						}
					}
				}
				if (mismatch) {
					printf "placeholder mismatch: %s en=(%s) de=(%s)\n", $1, join_set(en_placeholders), join_set(de_placeholders)
				}
			}
		' "$TMP/en.tsv" "$TMP/de.tsv" >> "$TMP/issues"
	fi

	[[ ! -s "$TMP/issues" ]]
}

report_issues() {
	local file="$1"
	{
		echo "LOCALE_CHECK status=fail"
		echo "  file: $file"
		sed 's/^/  /' "$TMP/issues"
	} >&2
}

# Legacy single-file mode: `LOCALE_CHECK keys=N status=ok`.
check_file() {
	local file="$1" min="${2:-}"
	if [[ -z "$min" ]]; then
		min="$(floor_for_file "$file")"
	fi
	if ! audit_file "$file" "$min"; then
		report_issues "$file"
		return 1
	fi
	echo "LOCALE_CHECK keys=$AUDIT_COUNT status=ok"
}

relative_to_root() {
	printf '%s\n' "${1#"$ROOT/"}"
}

# Default mode: both tables, independent floors, one deterministic line each.
check_all() {
	local failed=0 editor_count=0 world_count=0 content_count=0 status=ok

	if audit_file "$EDITOR_FILE" "$EDITOR_MIN_KEYS"; then
		editor_count="$AUDIT_COUNT"
		echo "LOCALE_CHECK file=$(relative_to_root "$EDITOR_FILE") keys=$editor_count min=$EDITOR_MIN_KEYS status=ok"
	else
		failed=1
		editor_count="$AUDIT_COUNT"
		echo "LOCALE_CHECK file=$(relative_to_root "$EDITOR_FILE") keys=$editor_count min=$EDITOR_MIN_KEYS status=fail"
		report_issues "$EDITOR_FILE"
	fi

	if audit_file "$WORLD_FILE" "$WORLD_MIN_KEYS"; then
		world_count="$AUDIT_COUNT"
		echo "LOCALE_CHECK file=$(relative_to_root "$WORLD_FILE") keys=$world_count min=$WORLD_MIN_KEYS status=ok"
	else
		failed=1
		world_count="$AUDIT_COUNT"
		echo "LOCALE_CHECK file=$(relative_to_root "$WORLD_FILE") keys=$world_count min=$WORLD_MIN_KEYS status=fail"
		report_issues "$WORLD_FILE"
	fi

	if audit_file "$CONTENT_FILE" "$CONTENT_MIN_KEYS"; then
		content_count="$AUDIT_COUNT"
		echo "LOCALE_CHECK file=$(relative_to_root "$CONTENT_FILE") keys=$content_count min=$CONTENT_MIN_KEYS status=ok"
	else
		failed=1
		content_count="$AUDIT_COUNT"
		echo "LOCALE_CHECK file=$(relative_to_root "$CONTENT_FILE") keys=$content_count min=$CONTENT_MIN_KEYS status=fail"
		report_issues "$CONTENT_FILE"
	fi

	if (( failed )); then
		status=fail
	fi
	echo "LOCALE_CHECK editor=$editor_count world=$world_count content=$content_count status=$status"
	(( ! failed ))
}

# --- self-test -------------------------------------------------------------

write_fixture() {
	local path="$1" keys="$2" marker="${3:-TABLES}" i
	{
		printf 'const %s := {\n\t"en": {\n' "$marker"
		for ((i = 1; i <= keys; i++)); do
			printf '\t\t"base.test.key_%03d": "Value %d {0} {name}",\n' "$i" "$i"
		done
		printf '\t},\n\t"de": {\n'
		for ((i = 1; i <= keys; i++)); do
			printf '\t\t"base.test.key_%03d": "Wert %d {0} {name}",\n' "$i" "$i"
		done
		printf '\t},\n}\n'
	} > "$path"
}

remove_key() {
	local file="$1" locale="$2" key="$3"
	awk -v loc="$locale" -v key="$key" '
		$0 == "\t\"" loc "\": {" { in_block = 1; print; next }
		in_block && $0 == "\t}," { in_block = 0; print; next }
		in_block && index($0, "\"" key "\": ") { next }
		{ print }
	' "$file"
}

set_value() {
	local file="$1" locale="$2" key="$3" value="$4"
	awk -v loc="$locale" -v key="$key" -v value="$value" '
		$0 == "\t\"" loc "\": {" { in_block = 1; print; next }
		in_block && $0 == "\t}," { in_block = 0; print; next }
		in_block && index($0, "\"" key "\": ") {
			printf "\t\t\"%s\": \"%s\",\n", key, value
			next
		}
		{ print }
	' "$file"
}

expect_pass() {
	local name="$1" file="$2" min="$3"
	local log="$TMP/self_$name.log"
	if ! "$SCRIPT" "$file" > "$log" 2>&1; then
		echo "LOCALE_SELF_TEST case=$name status=fail expected=pass" >&2
		sed 's/^/  /' "$log" >&2
		exit 1
	fi
	if ! grep -qx "LOCALE_CHECK keys=$min status=ok" "$log"; then
		echo "LOCALE_SELF_TEST case=$name status=fail reason=bad-marker" >&2
		sed 's/^/  /' "$log" >&2
		exit 1
	fi
	echo "LOCALE_SELF_TEST case=$name status=ok"
}

expect_fail() {
	local name="$1" file="$2" min="$3" pattern="$4"
	local log="$TMP/self_$name.log"
	if "$SCRIPT" "$file" > "$log" 2>&1; then
		echo "LOCALE_SELF_TEST case=$name status=fail expected=nonzero-exit" >&2
		sed 's/^/  /' "$log" >&2
		exit 1
	fi
	if ! grep -qF "$pattern" "$log"; then
		echo "LOCALE_SELF_TEST case=$name status=fail reason=missing-message" >&2
		echo "  expected: $pattern" >&2
		sed 's/^/  /' "$log" >&2
		exit 1
	fi
	echo "LOCALE_SELF_TEST case=$name status=ok"
}

self_test() {
	local dir="$TMP/fixtures" valid world_valid content_valid
	mkdir -p "$dir"
	valid="$dir/valid.gd"
	write_fixture "$valid" "$EDITOR_MIN_KEYS"

	expect_pass valid "$valid" "$EDITOR_MIN_KEYS"

	remove_key "$valid" de base.test.key_096 > "$dir/missing_de.gd"
	expect_fail missing_de "$dir/missing_de.gd" "$EDITOR_MIN_KEYS" \
		"missing in DE: base.test.key_096"

	awk '{ print } $0 == "\t\"de\": {" { print "\t\t\"base.test.extra\": \"Extra {0}\"," }' \
		"$valid" > "$dir/extra_de.gd"
	expect_fail extra_de "$dir/extra_de.gd" "$EDITOR_MIN_KEYS" \
		"missing in EN: base.test.extra"

	set_value "$valid" de base.test.key_005 "" > "$dir/empty_value.gd"
	expect_fail empty_value "$dir/empty_value.gd" "$EDITOR_MIN_KEYS" \
		"empty value: de base.test.key_005"

	set_value "$valid" de base.test.key_007 "Wert 7 {1} {name}" > "$dir/bad_placeholder.gd"
	expect_fail bad_placeholder "$dir/bad_placeholder.gd" "$EDITOR_MIN_KEYS" \
		"placeholder mismatch: base.test.key_007"

	write_fixture "$dir/too_few.gd" "$((EDITOR_MIN_KEYS - 1))"
	expect_fail too_few "$dir/too_few.gd" "$EDITOR_MIN_KEYS" \
		"minimum is $EDITOR_MIN_KEYS"

	# World table: same `TABLES` shape, independent lower floor. Every world
	# fixture keeps the real basename so the derived world floor is exercised.
	world_valid="$dir/world/world_i18n.gd"
	mkdir -p "$dir/world" "$dir/world_bad_placeholder" "$dir/world_too_few"
	write_fixture "$world_valid" "$WORLD_MIN_KEYS"
	expect_pass world_valid "$world_valid" "$WORLD_MIN_KEYS"

	set_value "$world_valid" de base.test.key_007 "Wert 7 {1} {name}" \
		> "$dir/world_bad_placeholder/world_i18n.gd"
	expect_fail world_bad_placeholder "$dir/world_bad_placeholder/world_i18n.gd" "$WORLD_MIN_KEYS" \
		"placeholder mismatch: base.test.key_007"

	write_fixture "$dir/world_too_few/world_i18n.gd" "$((WORLD_MIN_KEYS - 1))"
	expect_fail world_too_few "$dir/world_too_few/world_i18n.gd" "$WORLD_MIN_KEYS" \
		"minimum is $WORLD_MIN_KEYS"

	# Content table: `DISPLAY` marker, independent 32-key floor.
	content_valid="$dir/content/content_i18n.gd"
	mkdir -p "$dir/content" "$dir/content_bad_placeholder" "$dir/content_too_few"
	write_fixture "$content_valid" "$CONTENT_MIN_KEYS" DISPLAY
	expect_pass content_valid "$content_valid" "$CONTENT_MIN_KEYS"

	set_value "$content_valid" de base.test.key_007 "Wert 7 {1} {name}" \
		> "$dir/content_bad_placeholder/content_i18n.gd"
	expect_fail content_bad_placeholder "$dir/content_bad_placeholder/content_i18n.gd" "$CONTENT_MIN_KEYS" \
		"placeholder mismatch: base.test.key_007"

	write_fixture "$dir/content_too_few/content_i18n.gd" "$((CONTENT_MIN_KEYS - 1))" DISPLAY
	expect_fail content_too_few "$dir/content_too_few/content_i18n.gd" "$CONTENT_MIN_KEYS" \
		"minimum is $CONTENT_MIN_KEYS"

	echo "LOCALE_SELF_TEST cases=12 status=ok"
}

case "${1:-}" in
	--self-test)
		self_test
		;;
	-h | --help)
		usage
		;;
	-*)
		usage
		;;
	"")
		check_all
		;;
	*)
		check_file "$1"
		;;
esac
