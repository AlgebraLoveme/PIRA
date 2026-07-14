# `pira_codenav` test resources

The `synthetic/` fixtures are original PIRA test data licensed under the repository license.

The `real/` fixtures are pinned, unmodified source files used only for parser and performance coverage:

| Fixture | Upstream | Commit | Source license | SHA-256 |
|---|---|---|---|---|
| `real/python_click/decorators.py` | <https://github.com/pallets/click> | `b67832c2167e5b0ff6764a8c04a0a9087e697b5a` | BSD-3-Clause; see adjacent `LICENSE.txt` | `16069357615691fcdfc9c794bb515f6009eb35aad6f7a76c017bdce06dae8d55` |
| `real/rust_ripgrep/gitignore.rs` | <https://github.com/BurntSushi/ripgrep> | `d5b85d44057ff729a89be9c6549958c45d95aa99` | MIT; see adjacent `LICENSE-MIT` | `928d8bb106bc408e4b9b6208b3ae8e31e669cf70e9a3d8c7eb5113e968c85161` |
| `real/java_junit/StringUtils.java` | <https://github.com/junit-team/junit5> | `b87db9fe6616cefe4c03e165e61f54bd9c76017b` | EPL-2.0; see adjacent `LICENSE.md` | `706caaa9703f5e9a48803bb205854ec80f2f747a31785a1535928bbdbe7ccd04` |
| `real/c_jq/main.c` | <https://github.com/jqlang/jq> | `579e6f76cffd7643ba4002a2c3618a5ea710589a` | MIT; see adjacent `COPYING` | `4023f8b833982e1e6abace084995f7214bda7e54b8753c75d31c319d827cc263` |
| `real/cpp_fmt/format.cc` | <https://github.com/fmtlib/fmt> | `a79df4504cd4e42ed004b1113fb82171e62ed822` | MIT; see adjacent `LICENSE` | `5ccc4d8a363bd5c570782bc3dd92020664b1aa89de2d48b3b87d040c8c7bcc02` |
| `real/bash_bats/bats.sh` | <https://github.com/bats-core/bats-core> | `c18b2f7a7e56dde24b4a6ae706a4ecee3ec824ad` | MIT; see adjacent `LICENSE.md` | `7bcd3f732044c93ec9f7f6d6d861e80b5c1fe2b06c6a36222b6e45b7dc4e35e6` |
| `real/powershell_powershell/ResxGen.psm1` | <https://github.com/PowerShell/PowerShell> | `26757e26046ae5de93e489e62da1c8171ee23783` | MIT; see adjacent `LICENSE.txt` | `68f23dbc645a513f8bc9c80e99584bbed803ed8a5551d3a11bcd7a6cbbc0af3c` |
| `real/php_laravel/Collection.php` | <https://github.com/laravel/framework> | `3093ff3a61f88225f16fec35dda65e1e7c0867a4` | MIT; see adjacent `LICENSE.md` | `45e3c8080cf774426f44c99b7432d59a428b1adacd7a567a6ee0854a27148cfc` |
| `real/kotlin_coroutines/CoroutineDispatcher.kt` | <https://github.com/Kotlin/kotlinx.coroutines> | `165c6cb5859b5365dec193abc75dee9f49ce1389` | Apache-2.0; see adjacent `LICENSE.txt` | `26aa24947ea8d96797e1105595a54977a03cc715a3cce5a42e64cfb825b995a8` |
| `real/lua_neovim/lsp.lua` | <https://github.com/neovim/neovim> | `3a7989f4f4b2071bf436f730e363d0aee83ec1e5` | Apache-2.0 and Vim; see adjacent `LICENSE.txt` | `46067da650c8500dc353a24520972ec2941c1dc203840cfe64cd0a7d9ee29b83` |
| `real/hcl_terraform/root.tf` | <https://github.com/hashicorp/terraform> | `116d525a7242c2beb916f00ccb39ce070f4fac05` | BUSL-1.1; see adjacent `LICENSE` | `7bcab93b4804fd147ff1835611846b12d468fb4a54b8ee76b28528d0b98d6c4c` |
| `real/r_dplyr/mutate.R` | <https://github.com/tidyverse/dplyr> | `d5e94e7fa8fd4a5f79c1a707d1842216bb4c691f` | MIT; see adjacent `LICENSE` | `563acd4e01da98c6476447e01ba254e57c2b0cd23661835746ef8d7a0c07590e` |

The adjacent license files are copied from the same pinned commits. Tests never execute these files.
