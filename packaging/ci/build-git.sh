#!/usr/bin/env bash
# Builds one pinned git release from the kernel.org source tarball into PREFIX (the git-version axis of the platform
# matrix, ARCHITECTURE §13.1 L6 / §13.7; WBS WP-37). Used by .github/workflows/platform.yml on Linux runners; the
# result is cached per version, so a build happens only when the pin changes.
#
#   packaging/ci/build-git.sh VERSION PREFIX
#
# BUILD_GIT_JOBS sets make's job count. The default is min(nproc, 4) and the build runs at the lowest CPU priority, so
# that a local build on a shared development machine never starves concurrent work; CI runners have 2-4 cores anyway.
#
# Only the versions below are accepted, each with the SHA-256 of its tarball as listed in kernel.org's
# sha256sums.asc. Adding a version means adding its line here and to the matrix.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 VERSION PREFIX" >&2
  exit 2
fi
version=$1
prefix=$2

case "$version" in
  2.18.5) sha256=39768891b1deacb40287e041476a6186854f62691e55df951899267c1d3e5d5a ;;
  2.30.9) sha256=4e3985a641d9ebdfec9e56b370cdbe362a672d9b45270c1acb1406cdab0f5bb4 ;;
  2.31.8) sha256=c7a0c77fa63de669494b10a6b2a7f85d1e9ba69add501c89e035a36ded14d1d5 ;;
  2.34.8) sha256=600820b3b7fc2354f96d947ed5bc6755ac24560d4da972e62ebbb01e5e24583a ;;
  2.39.5) sha256=c58da92c378df4a986ca33266897a7397e86c22ee266a284d8c2432c39066b59 ;;
  2.43.7) sha256=657e2374455d9e62f6cdb3e7c55d867b6db5404d744e97e112cc5b0db687a19f ;;
  2.54.0) sha256=f689162364c10de79ef89aa8dbf48731eb057e34edbbd20aca510ce0154681a3 ;;
  2.56.0) sha256=26c56c296b38c0695b26fa95f475f1d01704d2d38e73465ca30b0b2f5dc789d3 ;;
  *)
    echo "build-git.sh: git $version is not pinned" >&2
    exit 2
    ;;
esac

cores=$(nproc 2> /dev/null || echo 1)
jobs=${BUILD_GIT_JOBS:-$((cores < 4 ? cores : 4))}
case "$jobs" in
  '' | *[!0-9]* | 0*)
    echo "build-git.sh: BUILD_GIT_JOBS must be a positive integer, not '$jobs'" >&2
    exit 2
    ;;
esac

if [ -x "$prefix/bin/git" ] && "$prefix/bin/git" --version | grep -qx "git version $version"; then
  echo "git $version already built in $prefix"
  exit 0
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tarball="git-$version.tar.xz"
curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 \
  --output "$work/$tarball" "https://mirrors.edge.kernel.org/pub/software/scm/git/$tarball"
echo "$sha256  $work/$tarball" | sha256sum --check --strict -
tar -xJf "$work/$tarball" -C "$work"

# Old releases predate GCC 10's -fno-common default; no Tcl/Tk, gettext, Perl or Python (the tests need none of the
# scripted porcelain); libcurl and expat for https remotes.
nice -n 19 make -C "$work/git-$version" -j"$jobs" \
  prefix="$prefix" \
  CFLAGS="-O2 -fcommon" \
  NO_TCLTK=1 NO_GETTEXT=1 NO_PERL=1 NO_PYTHON=1 NO_OPENSSL=1 NO_INSTALL_HARDLINKS=1 \
  all install

"$prefix/bin/git" --version
