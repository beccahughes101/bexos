# Third-Party Sources

## `linux_nvme`

`third_party/linux_nvme` is a shallow checkout of Andreas Hindborg's
Rust-for-Linux NVMe branch, pinned at
`18d8db987886ae3a257de0d18388b779231f4867` (`rnvme-v6.15`).

On case-insensitive filesystems, a full Linux checkout reports path collisions
for unrelated netfilter and memory-model files. Keep local checkouts sparse to
the reference areas used by BexOS:

```sh
git -C third_party/linux_nvme sparse-checkout init --no-cone
git -C third_party/linux_nvme sparse-checkout set /COPYING /LICENSES/ /drivers/nvme/ /rust/kernel/
```
