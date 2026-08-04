# Task 2 Report: Pure Parser Functions

## Implementation Summary

Implemented 4 public parser functions and 1 private helper in `src/theme.rs`:

- **`is_light(r: u16, g: u16, b: u16) -> bool`**: Weighted luminance calculation using ITU-R BT.709 coefficients (0.2126 R, 0.7152 G, 0.0722 B) with threshold 0.5 to classify terminal background as light or dark.

- **`parse_osc11(bytes: &[u8]) -> Option<(u16, u16, u16)>`**: Parses OSC 11 terminal background color replies in the format `ESC ] 11 ; rgb:RRRR/GGGG/BBBB` followed by BEL (\x07) or ST (ESC \) terminator. Returns scaled 16-bit RGB values or None on any parsing error.

- **`parse_hex_component(s: &str) -> Option<u16>`** (private): Helper function that parses 1–4 digit hexadecimal color components and scales them proportionally to the full 16-bit range (0–65535), not with zero-padding. Critical scaling: `f` (1-digit) → 0xffff, `ff` (2-digit) → 0xffff, `80` (2-digit) → ~0x82af (not 0x0080).

- **`parse_colorfgbg(s: &str) -> Option<Theme>`**: Parses COLORFGBG environment variable (format: `foreground;background` or `foreground;default;background`). Takes the last semicolon-separated segment as the background color index (0–15). Maps 0–6 and 8 → Dark, 7 and 9–15 → Light.

- **`theme_from_override(v: Option<&str>) -> Option<Theme>`**: Parses DCT_THEME environment variable with lenient handling: case-insensitive, trims whitespace. Accepts "dark" or "light" (case-insensitive); rejects invalid values including empty string, returning None to continue detection fallthrough.

## TDD Evidence

### RED Phase
Command: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme 2>&1 | head -20`

Output: Compilation failed with multiple `error[E0425]: cannot find function` errors for `is_light`, `parse_osc11`, `parse_colorfgbg`, `theme_from_override`.

**Why expected:** Functions not yet implemented; tests reference undefined identifiers.

### GREEN Phase
Command: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme 2>&1`

Output:
```
running 13 tests
test theme::tests::dark_gets_a_lighter_gray_than_light ... ok
test theme::tests::luminance_weights_are_not_transposed ... ok
test theme::tests::luminance_separates_real_terminal_backgrounds ... ok
test theme::tests::each_theme_has_a_distinct_dim_style ... ok
test theme::tests::parses_theme_override_leniently ... ok
test theme::tests::ignores_invalid_theme_override ... ok
test theme::tests::unknown_never_pins_a_foreground_color ... ok
test theme::tests::rejects_malformed_osc11_replies ... ok
test theme::tests::parses_colorfgbg ... ok
test theme::tests::rejects_malformed_colorfgbg ... ok
test theme::tests::parses_st_terminated_reply ... ok
test theme::tests::parses_four_digit_osc11_reply ... ok
test theme::tests::scales_short_hex_components_to_full_range ... ok

test result: ok. 13 passed; 0 failed
```

**Test Count:** 13 total (Task 1: 3 + Task 2: 10 new tests)

## Cargo Build Warnings

Command: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build 2>&1 | grep -E "warning|error"`

Output:
```
warning: function `is_light` is never used
warning: function `parse_osc11` is never used
warning: function `parse_hex_component` is never used
warning: function `parse_colorfgbg` is never used
warning: function `theme_from_override` is never used
warning: `dct` (lib) generated 5 warnings
```

**Analysis:** Exactly as expected per requirements:
- 4 `dead_code` warnings for the 4 `pub(crate)` functions (only called by tests at this checkpoint)
- 1 `dead_code` warning for the private helper `parse_hex_component` (only called by `parse_osc11`, which is dead code)
- **No other warnings** (no unused imports, no type warnings, no other errors)

These warnings will disappear when Task 3 wires the functions into `detect_with()`.

## Files Changed

- **`src/theme.rs`**: Added 4 public functions, 1 private helper, and 10 test cases to existing `#[cfg(test)] mod tests` block

## Commit

```
3fb468c feat: add pure parsers for OSC 11, COLORFGBG, and DCT_THEME
```

## Self-Review Findings

### Correctness Verified

1. **Luminance Formula:** 
   - Weights match ITU-R BT.709: 0.2126 (R), 0.7152 (G), 0.0722 (B) ✓
   - Threshold 0.5 separates Solarized Dark (≈0.14) and Light (≈0.97) cleanly ✓
   - No sRGB gamma correction (correctly identified as unnecessary for classification) ✓

2. **OSC 11 Parsing:**
   - Correctly handles both BEL (\x07) and ST (ESC \) terminators ✓
   - Rejects replies without `rgb:` prefix ✓
   - Rejects incomplete data (missing terminator, truncated channels) ✓
   - Validates exactly 3 color components, rejects extra segments ✓

3. **Hex Component Scaling (Critical):**
   - Formula: `(v * u16::MAX / max)` where `max = 16^len - 1` ✓
   - 1-digit `f`: (15 * 65535 / 15) = 65535 ✓
   - 2-digit `80`: (128 * 65535 / 255) ≈ 32893, which is > 0x8000 (32768) and < 0x8100 (33024) ✓
   - Correctly uses proportion scaling, not zero-padding ✓
   - No integer overflow: u32 used for intermediate calculation before cast to u16 ✓

4. **COLORFGBG Parsing:**
   - Correctly takes **last** segment after splitting on `;` ✓
   - Validates segment is numeric (0–15 range only) ✓
   - Maps 0–6, 8 → Dark; 7, 9–15 → Light ✓
   - Rejects values outside 0–15 (e.g., 999) ✓
   - Rejects non-numeric segments (e.g., "default" in wrong position) ✓

5. **Theme Override Parsing:**
   - Case-insensitive matching with `.to_ascii_lowercase()` ✓
   - Trims leading/trailing whitespace ✓
   - Returns None (not Some(Dark)) for invalid values—critical for preventing silent misconfiguration ✓
   - Handles None input correctly ✓

6. **Error Handling:**
   - All functions gracefully degrade with Option return types; no panics ✓
   - Invalid UTF-8 in OSC 11 → None ✓
   - Malformed hex → None ✓
   - Out-of-range values → None ✓

### Code Quality

- **Comments:** Written in Chinese as required, explain WHY not WHAT (e.g., explaining why sRGB gamma is skipped, why scaling must be proportional) ✓
- **Naming:** Clear and matches brief exactly ✓
- **Test Coverage:** 10 new tests cover happy paths, edge cases, and error scenarios ✓
- **No Dead Code Introduced:** All functions exist only because brief requires them; no internal helpers beyond `parse_hex_component` ✓

### No Known Issues

- No panics possible in non-test code paths
- No unsafe code
- No external dependencies added
- Consistent with Task 1's Theme enum design
- Ready for Task 3 integration

