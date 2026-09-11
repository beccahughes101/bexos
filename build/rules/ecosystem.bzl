EcosystemInfo = provider(
    fields = {
        "profile": "ecosystem profile name",
        "deployable": "whether this profile may perform production remote mutation",
        "policy": "distribution policy file",
        "tls_roots": "TLS root redb",
        "app_roots": "app-signing root redb",
        "tuf_root": "TUF root metadata",
        "direct_update_keys": "direct-update public keys",
        "public_bundle": "BootFS public ecosystem bundle",
        "signing_key": "private signing material label, not embedded in public_bundle",
    },
)

AppSignerInfo = provider(
    fields = {
        "key": "private key file or external signer descriptor",
        "chain": "public signing certificate chain",
        "template_only": "true when development material is only allowed for template assembly",
    },
)

def _ecosystem_profile_impl(ctx):
    if not ctx.attr.profile:
        fail("ecosystem profile must have a non-empty identity")
    if ctx.attr.profile == "prod" and ctx.attr.deployable and ctx.file.signing_key:
        fail("deployable production profiles must not depend on checked-in signing material")
    if ctx.attr.profile != "prod" and not ctx.file.signing_key:
        fail("development profiles must provide signing material")
    if ctx.attr.profile == "prod" and not ctx.attr.template_only and ctx.file.signing_key:
        fail("production profiles that use development signing material must be template_only")
    return [
        EcosystemInfo(
            profile = ctx.attr.profile,
            deployable = ctx.attr.deployable,
            policy = ctx.file.policy,
            tls_roots = ctx.file.tls_roots,
            app_roots = ctx.file.app_roots,
            tuf_root = ctx.file.tuf_root,
            direct_update_keys = ctx.file.direct_update_keys,
            public_bundle = ctx.file.public_bundle,
            signing_key = ctx.file.signing_key,
        ),
        AppSignerInfo(
            key = ctx.file.signing_key,
            chain = ctx.files.signing_chain,
            template_only = ctx.attr.template_only,
        ),
        DefaultInfo(files = depset([ctx.file.public_bundle])),
    ]

ecosystem_profile = rule(
    implementation = _ecosystem_profile_impl,
    attrs = {
        "profile": attr.string(mandatory = True),
        "deployable": attr.bool(default = False),
        "template_only": attr.bool(default = False),
        "policy": attr.label(allow_single_file = True, mandatory = True),
        "tls_roots": attr.label(allow_single_file = True, mandatory = True),
        "app_roots": attr.label(allow_single_file = True, mandatory = True),
        "tuf_root": attr.label(allow_single_file = True, mandatory = True),
        "direct_update_keys": attr.label(allow_single_file = True, mandatory = True),
        "public_bundle": attr.label(allow_single_file = True, mandatory = True),
        "signing_key": attr.label(allow_single_file = True),
        "signing_chain": attr.label_list(allow_files = True),
    },
)
