#!/usr/bin/env bash
# Decides whether the release workflow signs the Firefox extension on
# addons.mozilla.org (#228). Writes `sign=true|false` and `version=<app
# version>` to $GITHUB_OUTPUT and explains a skip with a ::notice:: — a
# skip never fails the build.
#
# Signs only when ALL hold:
#   - the run is for a tag (GITHUB_REF=refs/tags/...): a build-only manual
#     run never signs, since AMO keeps every version number it signs forever;
#   - both AMO_JWT_ISSUER and AMO_JWT_SECRET are set (repository secrets);
#   - the app version is a plain X.Y.Z: the Firefox manifest drops a
#     pre-release suffix, so 0.10.1-rc.1 would burn 0.10.1 on AMO.
#
# Inputs (env): GITHUB_REF, AMO_JWT_ISSUER, AMO_JWT_SECRET, GITHUB_OUTPUT,
# and optionally APP_VERSION (default: read from sussurro/package.json).
# Dry run:  GITHUB_REF=refs/heads/main GITHUB_OUTPUT=/dev/stdout bash extension/scripts/amo-sign-gate.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version="${APP_VERSION:-$(node -p "require('$root/sussurro/package.json').version")}"
out="${GITHUB_OUTPUT:?GITHUB_OUTPUT is not set}"

sign=false
if [[ "${GITHUB_REF:-}" != refs/tags/* ]]; then
  echo "::notice title=Firefox signing skipped::Not a tag run (${GITHUB_REF:-no ref}): the extension is built but not signed on addons.mozilla.org."
elif [[ -z "${AMO_JWT_ISSUER:-}" || -z "${AMO_JWT_SECRET:-}" ]]; then
  echo "::notice title=Firefox signing skipped::AMO_JWT_ISSUER / AMO_JWT_SECRET secrets are not set: only the unsigned Firefox zip is attached (see docs/releases.md)."
elif [[ ! "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
  echo "::notice title=Firefox signing skipped::Version $version is not a plain X.Y.Z: pre-releases are not signed (AMO would reserve the X.Y.Z version for good)."
else
  sign=true
fi

{
  echo "sign=$sign"
  echo "version=$version"
} >> "$out"
