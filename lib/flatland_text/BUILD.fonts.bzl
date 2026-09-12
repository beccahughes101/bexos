"""Pinned, licensed font inputs for consumers which embed deterministic text."""
FONT_DATA = ["@inter//file", "@jetbrains_mono//file", "@noto_sans//file", "@noto_arabic//file", "@noto_devanagari//file"]
FONT_ENV = {
    "INTER": "$(execpath @inter//file)",
    "JETBRAINS_MONO": "$(execpath @jetbrains_mono//file)",
    "NOTO_SANS": "$(execpath @noto_sans//file)",
    "NOTO_ARABIC": "$(execpath @noto_arabic//file)",
    "NOTO_DEVANAGARI": "$(execpath @noto_devanagari//file)",
}
FONT_LICENSES = {
    "licenses/Inter-OFL.txt": "@inter_license//file",
    "licenses/JetBrainsMono-OFL.txt": "@jetbrains_mono_license//file",
    "licenses/NotoSans-OFL.txt": "@noto_license//file",
    "licenses/NotoSansArabic-OFL.txt": "@noto_arabic_license//file",
    "licenses/NotoSansDevanagari-OFL.txt": "@noto_devanagari_license//file",
}

SYSTEM_FONT_ENTRIES = {
    "data/fonts/InterVariable.ttf": "@inter//file",
    "data/fonts/JetBrainsMonoVariable.ttf": "@jetbrains_mono//file",
    "data/fonts/NotoSansVariable.ttf": "@noto_sans//file",
    "data/fonts/NotoSansArabicVariable.ttf": "@noto_arabic//file",
    "data/fonts/NotoSansDevanagariVariable.ttf": "@noto_devanagari//file",
}
