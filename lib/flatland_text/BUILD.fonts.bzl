"""Pinned, licensed font inputs for consumers which embed deterministic text."""
FONT_DATA = ["@noto_sans//file", "@noto_arabic//file", "@noto_devanagari//file"]
FONT_ENV = {
    "NOTO_SANS": "$(execpath @noto_sans//file)",
    "NOTO_ARABIC": "$(execpath @noto_arabic//file)",
    "NOTO_DEVANAGARI": "$(execpath @noto_devanagari//file)",
}
FONT_LICENSES = {
    "licenses/NotoSans-OFL.txt": "@noto_license//file",
    "licenses/NotoSansArabic-OFL.txt": "@noto_arabic_license//file",
    "licenses/NotoSansDevanagari-OFL.txt": "@noto_devanagari_license//file",
}
