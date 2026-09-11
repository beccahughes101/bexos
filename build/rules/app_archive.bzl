def app_archive(name, manifest, entries = {}, compression = "none", signing_key = "//ecosystem/bexos:app_signing_key", config = None, config_policy = None):
    """Builds a signed .bex app archive.

    Args:
      name: target name.
      manifest: compiled .bexmanifest label.
      entries: dict of archive-relative path to source label.
      compression: "none" or "zstd".
      signing_key: AppSignerInfo-compatible key label containing key_id_hex and seed_hex fields.
      config: optional compiled component config blob.
    """
    all_entries = dict(entries)
    if config:
        all_entries["config/component.bexconfig"] = config
    if config_policy:
        all_entries["config/component.bexpolicy"] = config_policy
    srcs = [manifest, signing_key] + all_entries.values()
    cmd = "$(location //tools/app_archive:bex_archive) create --out $@ --manifest $(location %s) --key $(location %s) --compression %s" % (
        manifest,
        signing_key,
        compression,
    )
    for archive_path, label in sorted(all_entries.items()):
        cmd += " --entry %s=$(location %s)" % (archive_path, label)
    native.genrule(
        name = name,
        srcs = srcs,
        outs = [name + ".bex"],
        cmd = cmd,
        tools = ["//tools/app_archive:bex_archive"],
    )
