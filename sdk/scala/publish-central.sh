#!/usr/bin/env bash
#
# Upload the signed bundle `sbt publishSigned` staged to Maven Central, through
# the Central Portal's publisher API.
#
# Why this exists rather than `sbt sonatypeBundleRelease`: sbt-sonatype (3.12.2,
# the newest) speaks the legacy Nexus staging API. The Portal does not serve it
# — `https://central.sonatype.com/service/local/...` returns a 404 HTML page —
# and Sonatype's compatibility host implements only part of it, so the release
# path dies on
#
#   400 Bad Request: Endpoint /service/local/staging/profile_repositories
#                    not supported
#
# The Portal's own API takes the whole bundle as one zip, which is exactly what
# is already staged. No plugin in between, so nothing here breaks when the next
# one changes.
#
# Credentials come from a Central Portal **user token** (central.sonatype.com ->
# your account -> Generate User Token), via the environment:
#
#   CENTRAL_TOKEN_USERNAME, CENTRAL_TOKEN_PASSWORD
#
# Usage:
#   sbt publishSigned && ./publish-central.sh
#   PUBLISHING_TYPE=AUTOMATIC ./publish-central.sh   # release without the portal UI step
set -euo pipefail

cd "$(dirname "$0")"

VERSION="${VERSION:-$(sed -n 's/^ThisBuild \/ version := "\(.*\)"/\1/p' build.sbt | head -1)}"
STAGE="target/sonatype-staging/${VERSION}"
# USER_MANAGED drops the deployment in the portal for you to eyeball and release
# by hand. AUTOMATIC publishes the moment validation passes — no undo, so it is
# opt-in.
PUBLISHING_TYPE="${PUBLISHING_TYPE:-USER_MANAGED}"
API="https://central.sonatype.com/api/v1/publisher/upload"

if [ ! -d "$STAGE" ]; then
  echo "no staged bundle at $STAGE — run 'sbt publishSigned' first" >&2
  exit 2
fi

: "${CENTRAL_TOKEN_USERNAME:?set CENTRAL_TOKEN_USERNAME (central.sonatype.com -> Generate User Token)}"
: "${CENTRAL_TOKEN_PASSWORD:?set CENTRAL_TOKEN_PASSWORD (central.sonatype.com -> Generate User Token)}"

# Every artifact must be signed, and Central checks. Catching it here turns a
# rejected upload into a message that names the file.
missing=0
while IFS= read -r f; do
  case "$f" in *.asc|*.md5|*.sha1) continue ;; esac
  [ -f "$f.asc" ] || { echo "unsigned: $f" >&2; missing=1; }
done < <(find "$STAGE" -type f)
[ "$missing" -eq 0 ] || { echo "refusing to upload an unsigned bundle" >&2; exit 1; }

BUNDLE="$(pwd)/target/central-bundle-${VERSION}.zip"
rm -f "$BUNDLE"
# Zip from inside the stage dir: the Portal expects the Maven layout
# (io/github/...) at the archive root, not nested under a staging directory.
( cd "$STAGE" && zip -qr "$BUNDLE" . )
echo "bundle: $BUNDLE ($(du -h "$BUNDLE" | cut -f1))"

NAME="$(sed -n 's/^ *name := "\(.*\)",*/\1/p' build.sbt | head -1)-${VERSION}"
AUTH="$(printf '%s:%s' "$CENTRAL_TOKEN_USERNAME" "$CENTRAL_TOKEN_PASSWORD" | base64)"

echo "uploading as '${NAME}' (${PUBLISHING_TYPE})..."
# --fail-with-body so a rejection prints Central's reason rather than just an
# exit code; the body is the only place it explains itself.
HTTP=$(curl -sS --fail-with-body -w '\n%{http_code}' \
  --connect-timeout 30 --max-time "${UPLOAD_TIMEOUT:-900}" \
  -H "Authorization: Bearer ${AUTH}" \
  -F "bundle=@${BUNDLE}" \
  "${API}?name=$(printf '%s' "$NAME" | sed 's/ /%20/g')&publishingType=${PUBLISHING_TYPE}") || {
    echo "$HTTP" >&2
    echo "upload failed — see the response above" >&2
    exit 1
  }

BODY="$(printf '%s' "$HTTP" | sed '$d')"
CODE="$(printf '%s' "$HTTP" | tail -1)"
echo "HTTP $CODE"
[ -n "$BODY" ] && echo "deployment id: $BODY"

if [ "$PUBLISHING_TYPE" = "USER_MANAGED" ]; then
  echo
  echo "Staged. Review and release it at:"
  echo "  https://central.sonatype.com/publishing/deployments"
fi
