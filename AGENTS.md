Always use bazel including for proto generation and never lie to me about fixing something

Never leave servers running except bazel

Always deliver on your committments and never leave something half done (be clear about what you did)

I prefer not putting everything in a single file and breaking it up into multiple files and shared libraries.

Bazel (for all builds and codegen, do not commit generated files)

Always run bazel run @rules_rust//:rustfmt to clean up rust

When updating docs update state but don't delete future designs if present in the doc (docs/rfcs/*/README.md is the full long term design; docs and docs/rfcs/*/CURRENT.md should describe current state)

App manifests should always be prototxt (as with all config)

You should not be building drivers and services without heart transplant support

When we are working on something when you have an approved plan don't stop until you've delivered on every point in the plan

Ask before you go on an unrelated detour

Always write code first and then run the tests, they take a long time

