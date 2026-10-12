# Fennel compiler asset

Unmodified official Fennel **1.6.1** library and MIT license, copied from the
caller's pre-fetched source; nothing is downloaded at build or runtime.

- Upstream commit: `9af5e47fa7ddea806544d15507412ddc51bd046d`
- Library: https://fennel-lang.org/downloads/fennel-1.6.1.lua
- Library SHA256: `c3d45602041e7d8ef8a212563573df040c48a85c648a29fb4597ebed4bc38ec2`
- License: https://github.com/bakpakin/Fennel/blob/9af5e47fa7ddea806544d15507412ddc51bd046d/LICENSE
- License SHA256: `508a77d2e7b51d98adeed32648ad124b7b30241a8e70b2e72c99f92d8e5874d1`

The outer caller reports verification of the official SSH signature for
`phil@hagelb.org`, published key fingerprint
`SHA256:BQRmdwPKmDaiWjbDUQDVxdftnDqIkgk/3heaYNU6hrQ`. Signature and key came
from the same official origin. This implementation checked byte hashes; it did
not independently repeat the signature verification or fetch upstream content.

Only `workflow-fennel-spike` embeds the library. Integration code lives in
`assets/workflow/compile-fennel.lua` and `src/workflow/fennel.rs`. Changes belong
there, never in the vendored library. See `docs/workflow-fennel-spike.md` for
compiler-state restrictions, process bounds and pinned source-map coupling.
