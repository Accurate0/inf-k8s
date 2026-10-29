# ortzi

Pluggable CLI for the platform. Packages are native binaries published to the registry at
`registry.ortzi.cloud` (`platform-services/registry`) and run as `ortzi <package> ...`.

## Using it

```sh
just platform install        # first install, from source; afterwards `ortzi self-update`
ortzi login
ortzi search
ortzi install ff ai janitor
ortzi ff flag list           # logs in to the package's own service when needed
ortzi update                 # ortzi itself and every installed package
```

Optionally put `$(ortzi bin-dir)` on `PATH` to run installed binaries (`ffctl`, `aig`, ...) directly.

## Publishing a package

```sh
cd path/to/your/cli
ortzi init                   # writes ortzi.yaml and .forgejo/workflows/publish-cli-<name>.yaml
```

A registry admin reserves the name and trusts the repo, once:

```sh
ortzi registry package create <name> --forgejo-repo <owner/repo> \
  --ref refs/heads/main --workflow publish-cli-<name>.yaml
```

Pushing to `main` then publishes whenever `version` in `ortzi.yaml` is bumped; an
already-published version is skipped. The
workflow runs on the `ortzi` runner label, which has cargo-zigbuild and both targets
(`darwin-arm64`, `linux-amd64`), and authenticates with Forgejo Actions OIDC, so no
secrets are needed. For anywhere else, mint a scoped token with
`ortzi registry token create --package <name>` and set `ORTZI_TOKEN`.

### ortzi.yaml

```yaml
package:
  name: ff                     # `ortzi ff`
  description: feature-flags admin CLI
  binary: ffctl                # defaults to name
  aliases: [ffctl]             # extra `ortzi <alias>` names
  version: 0.1.0               # bump to publish; falls back to the crate version
auth:                          # optional: ortzi logs in and passes the token
  client_id: feature-flags     # issuer defaults to https://idm.anurag.sh/oauth2/openid/<client_id>
  token_env: FFCTL_TOKEN       # env var the binary reads its bearer token from
build:
  kind: cargo                  # or prebuilt
  targets: [darwin-arm64, linux-amd64]
  cargo:
    package: ffctl             # -p, defaults to binary
  # prebuilt:
  #   linux-amd64: dist/foo-${version}-linux-amd64
```

`ortzi publish --dry-run` builds without uploading.
