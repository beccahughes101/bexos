"""Compile app-local FTL sources into a validated, indexed BEXRES catalog."""

LocaleListInfo = provider(fields = ["values"])

def _bundle_impl(ctx):
    output = ctx.actions.declare_file(ctx.label.name + "/strings.bexres")
    args = ctx.actions.args()
    args.add(output)
    args.add(ctx.attr.default_locale)
    args.add_all(ctx.files.srcs)
    ctx.actions.run(
        executable = ctx.executable._compiler,
        inputs = ctx.files.srcs,
        outputs = [output],
        arguments = [args],
        mnemonic = "FluentCompile",
    )
    return [DefaultInfo(files = depset([output]))]

bexos_locale_bundle = rule(
    implementation = _bundle_impl,
    attrs = {
        "srcs": attr.label_list(allow_files = [".ftl"], mandatory = True),
        "default_locale": attr.string(default = "en-US"),
        "_compiler": attr.label(default = "//tools/locale_bundle", executable = True, cfg = "exec"),
    },
)

def _cldr_impl(ctx):
    output = ctx.actions.declare_file(ctx.label.name + "/cldr.bexloc")
    args = ctx.actions.args()
    args.add(output)
    args.add(ctx.file._source)
    args.add(ctx.file._icu_source)
    args.add_all(ctx.attr.locale_config[LocaleListInfo].values if ctx.attr.locale_config else ctx.attr.locales)
    ctx.actions.run(
        executable = ctx.executable._compiler,
        inputs = [ctx.file._source, ctx.file._icu_source],
        outputs = [output],
        arguments = [args],
        mnemonic = "CldrExport",
    )
    return [DefaultInfo(files = depset([output]))]

bexos_cldr_data = rule(
    implementation = _cldr_impl,
    attrs = {
        "locales": attr.string_list(default = ["en-US"]),
        "locale_config": attr.label(providers = [LocaleListInfo]),
        "_source": attr.label(default = "@locale_cldr_source//file", allow_single_file = True),
        "_icu_source": attr.label(default = "@locale_icu_source//file", allow_single_file = True),
        "_compiler": attr.label(default = "//tools/cldr_data", executable = True, cfg = "exec"),
    },
)

def _locale_list_impl(ctx):
    values = ctx.build_setting_value
    if not values or "en-US" not in values or len(values) > 64:
        fail("CLDR locales must include en-US and contain at most 64 tags")
    return [LocaleListInfo(values = values)]

locale_list = rule(
    implementation = _locale_list_impl,
    build_setting = config.string_list(flag = True),
)
