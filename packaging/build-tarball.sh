#!/usr/bin/env bash
# A distribution-agnostic binary tarball: everything an install needs, and a script to place it.
#
# For someone whose distribution is not Arch, Debian or Fedora, or who does not want a package
# manager involved. It installs under /usr/local by default, which is where a hand-installed
# program belongs and which no package manager will fight over.
#
#   packaging/build-tarball.sh [version]
#
# Produces dist/crossfx-<version>-<arch>.tar.gz beside the repository. This is also what the
# release workflow attaches: it used to assemble its own archive inline, with a different
# desktop-file name and doc directory, and the -bin AUR recipe was written against that one —
# so there is exactly one layout now, this script's, and it is the layout packaging/PKGBUILD
# installs, minus the /usr prefix:
#
#   bin/crossfx
#   lib/systemd/user/crossfx.service
#   share/dbus-1/services/io.github.blackixxce12.CrossFX.service
#   share/applications/io.github.blackixxce12.CrossFX.desktop
#   share/icons/hicolor/{256x256,32x32}/apps/io.github.blackixxce12.CrossFX.png
#   share/icons/hicolor/scalable/status/io.github.blackixxce12.CrossFX-{off,on,processing}.svg
#   share/crossfx/presets/{Factsoft,BonusPresets}/*.fac  share/crossfx/presets/Input/*.toml
#   share/man/man1/crossfx.1
#   share/metainfo/io.github.blackixxce12.CrossFX.metainfo.xml
#   share/doc/crossfx/{README.md,CHANGELOG.md,LICENSE,COPYING.rnnoise,OFL.noto,OFL.manrope,
#                      hyprland.conf.example,crossfx-autostart.desktop}
#   install.sh
#
# The binary is always built from the tree the script sits in, and one whose `--version` is not
# that tree's version is refused rather than packed.
#
# The binary is linked against the glibc of the machine that builds it and runs on that release
# and newer. CI builds it on debian:bookworm for that reason; a local build is for local use.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# The first `version =` line of Cargo.toml is [workspace.package]'s; no dependency line starts
# with the word, which is what keeps this one-liner honest.
crate_version="$(sed -n 's/^version = "\(.*\)"/\1/p' "${root}/Cargo.toml" | head -1)"
version="${1:-${crate_version}}"
arch="$(uname -m)"
stage="$(mktemp -d)"
name="crossfx-${version}-${arch}"
trap 'rm -rf "${stage}"' EXIT

cd "${root}"
# Always build, even when target/release/crossfx is already there: a binary an earlier checkout
# left behind — a 0.3.0 built before pulling 0.4.0 — would otherwise be packed under this tree's
# version, beside presets, a unit and a D-Bus activation file it does not understand. When the
# binary is current, cargo does nothing. The target directory is pinned for this one command, so
# that a CARGO_TARGET_DIR or build.target-dir set elsewhere cannot send the build somewhere this
# script does not look.
CARGO_TARGET_DIR="${root}/target" cargo build --release --locked --bin crossfx
bin="${root}/target/release/crossfx"
# And the binary has to be this tree's: `crossfx --version` prints "crossfx <version>".
built="$("${bin}" --version)"
if [ "${built}" != "crossfx ${crate_version}" ]; then
  echo "build-tarball.sh: ${bin} says '${built}', but Cargo.toml is at ${crate_version};" \
    "refusing to pack it" >&2
  exit 1
fi

pkg="${stage}/${name}"
install -Dm755 "${bin}" "${pkg}/bin/crossfx"
# Named after the window's app_id (StartupWMClass=io.github.blackixxce12.CrossFX), so the
# compositor can match the window to the entry: the name every package installs it under.
install -Dm644 packaging/io.github.blackixxce12.CrossFX.desktop \
  "${pkg}/share/applications/io.github.blackixxce12.CrossFX.desktop"
# Installed as written, with ExecStart=/usr/bin/crossfx; install.sh below rewrites that for the
# prefix it is given, because a unit that points at /usr/bin on a /usr/local install starts nothing.
install -Dm644 packaging/crossfx.service "${pkg}/lib/systemd/user/crossfx.service"
# D-Bus activation, with Exec=/usr/bin/crossfx rewritten by install.sh as the unit is.
install -Dm644 packaging/io.github.blackixxce12.CrossFX.service \
  "${pkg}/share/dbus-1/services/io.github.blackixxce12.CrossFX.service"
# The two icon sizes the tree actually carries, at the paths packaging/PKGBUILD uses, under the
# application id: the one name the desktop entry, the notifications
# (crates/fxsound-app/src/notify.rs) and the sound settings look the icon up by.
install -Dm644 assets/images/fxsound_large.png \
  "${pkg}/share/icons/hicolor/256x256/apps/io.github.blackixxce12.CrossFX.png"
install -Dm644 assets/images/fxsound.png \
  "${pkg}/share/icons/hicolor/32x32/apps/io.github.blackixxce12.CrossFX.png"
# The tray icon's three states, resolved by name through the theme; without them the tray shows
# a blank where the icon should be.
install -Dm644 assets/icons/status/io.github.blackixxce12.CrossFX-{off,on,processing}.svg \
  -t "${pkg}/share/icons/hicolor/scalable/status/"
# Both preset trees: the .fac files the original FxSound for Windows shares, and the TOML voice
# presets.
for dir in Factsoft BonusPresets; do
  install -d "${pkg}/share/crossfx/presets/${dir}"
  install -Dm644 assets/presets/"${dir}"/*.fac -t "${pkg}/share/crossfx/presets/${dir}/"
done
install -d "${pkg}/share/crossfx/presets/Input"
install -Dm644 assets/presets/Input/*.toml -t "${pkg}/share/crossfx/presets/Input/"
# The unit says Documentation=man:crossfx(1); shipping the page is what makes that true.
install -Dm644 packaging/crossfx.1 "${pkg}/share/man/man1/crossfx.1"
install -Dm644 packaging/io.github.blackixxce12.CrossFX.metainfo.xml \
  "${pkg}/share/metainfo/io.github.blackixxce12.CrossFX.metainfo.xml"
# share/doc/crossfx: the package name, as on every distribution.
install -Dm644 README.md CHANGELOG.md LICENSE \
  packaging/hyprland.conf.example packaging/crossfx-autostart.desktop \
  -t "${pkg}/share/doc/crossfx/"
# Beside the AGPL, the notices of the three parts of the binary that are not AGPL, whose licences
# make shipping the text a condition of shipping the binary: the RNNoise code and model
# (BSD-3-Clause), the embedded Noto fallback faces and the Manrope interface faces (both SIL OFL
# 1.1, each with its own notice). crossfx-bin installs all four from here into
# /usr/share/licenses.
install -Dm644 crates/fxsound-rnnoise/COPYING "${pkg}/share/doc/crossfx/COPYING.rnnoise"
install -Dm644 assets/fonts/OFL.txt "${pkg}/share/doc/crossfx/OFL.noto"
install -Dm644 assets/fonts/OFL-Manrope.txt "${pkg}/share/doc/crossfx/OFL.manrope"

cat > "${pkg}/install.sh" <<'INNER'
#!/usr/bin/env sh
# Copy this tree into a prefix. Default /usr/local; pass another as the first argument.
set -eu
prefix="${1:-/usr/local}"
here="$(cd "$(dirname "$0")" && pwd)"

# The directory PATH names, with its symlinks and `..` resolved as far as it exists: that part
# through `pwd -P`, the rest by its names, each `..` among them taking one name back.
resolved() {
  r_path=$1
  case "${r_path}" in
    /*) ;;
    *) r_path="$(pwd)/${r_path}" ;;
  esac
  r_head=${r_path}
  r_tail=
  while [ ! -d "${r_head}" ]; do
    r_tail="/$(basename -- "${r_head}")${r_tail}"
    r_head="$(dirname -- "${r_head}")"
  done
  r_out="$(cd -P -- "${r_head}" 2>/dev/null && pwd -P)" || r_out=${r_head}
  r_ifs=${IFS}
  IFS=/
  set -f
  for r_name in ${r_tail}; do
    case "${r_name}" in
      '' | .) ;;
      ..) r_out="$(dirname -- "${r_out}")" ;;
      *) r_out="${r_out%/}/${r_name}" ;;
    esac
  done
  set +f
  IFS=${r_ifs}
  printf '%s\n' "${r_out}"
}

# Whether the resolved directory DIR is the resolved directory ROOT or lies under it.
inside() {
  case "$1/" in
    "${2%/}"/*) return 0 ;;
  esac
  return 1
}

case "${prefix}" in
  /usr|/usr/local) ;;
  *)
    # Not into a home folder. CrossFX keeps a user's own presets, voice presets and unsaved
    # edits in their data home's crossfx/presets, which is <prefix>/share/crossfx/presets for a
    # prefix such as ~/.local: the copy below would lay the factory presets over every one of
    # theirs that shares a name, and the binary, which looks for factory presets only under /usr
    # and /usr/local, would list none of them as such. Under sudo, $HOME is normally root's, so
    # the home of the user who ran it comes from the password database.
    if [ -n "${SUDO_USER:-}" ]; then
      user=${SUDO_USER}
      home="$(getent passwd "${SUDO_USER}" 2>/dev/null | cut -d: -f6)" || home=
      data_home="${home}/.local/share"
    else
      user="${USER:-$(id -un 2>/dev/null || echo this user)}"
      home=${HOME:-}
      case "${XDG_DATA_HOME:-}" in
        /*) data_home=${XDG_DATA_HOME} ;;
        *) data_home="${home}/.local/share" ;;
      esac
    fi
    case "${home}" in
      /*) ;;
      *)
        echo "refusing to install into ${prefix}: it cannot tell where ${user}'s home is," \
          "where CrossFX keeps that user's own presets. CrossFX finds its factory presets" \
          "only under /usr and /usr/local: run 'sudo ${here}/install.sh' to install into" \
          "/usr/local." >&2
        exit 1
        ;;
    esac
    target="$(resolved "${prefix}")"
    refuse=
    if inside "${target}" "$(resolved "${home}")"; then
      refuse=1
    fi
    case "${HOME:-}" in
      /*)
        if inside "${target}" "$(resolved "${HOME}")"; then
          refuse=1
        fi
        ;;
    esac
    if [ "$(resolved "${target}/share")" = "$(resolved "${data_home}")" ]; then
      refuse=1
    fi
    if [ -n "${refuse}" ]; then
      echo "refusing to install into ${prefix}: it is inside ${user}'s home, where CrossFX" \
        "keeps that user's own presets (${data_home}/crossfx). CrossFX finds its factory" \
        "presets only under /usr and /usr/local: run 'sudo ${here}/install.sh' to install" \
        "into /usr/local." >&2
      exit 1
    fi
    # Not a refusal, because there are reasons to put a binary elsewhere — but the presets will
    # not be found there: the binary searches exactly /usr/share/crossfx and
    # /usr/local/share/crossfx (install_data_dirs in crates/fxsound-preset/src/lib.rs), not
    # $XDG_DATA_DIRS, and systemd --user does not look for units under an arbitrary prefix either.
    echo "warning: CrossFX only looks for its presets under /usr and /usr/local; installed" >&2
    echo "         under ${prefix} it will start with an empty preset list, and systemd will" >&2
    echo "         not find ${prefix}/lib/systemd/user/crossfx.service." >&2
    ;;
esac

echo "installing CrossFX into ${prefix}"
mkdir -p "${prefix}"
cp -a "${here}/bin" "${here}/share" "${prefix}/"
[ -d "${here}/lib" ] && cp -a "${here}/lib" "${prefix}/"

# The unit and the D-Bus activation file ship with the path every package uses. Point them at
# the binary that was just installed; on a /usr prefix this changes nothing.
for started in "${prefix}/lib/systemd/user/crossfx.service" \
  "${prefix}/share/dbus-1/services/io.github.blackixxce12.CrossFX.service"; do
  if [ -f "${started}" ] && [ "${prefix}" != /usr ]; then
    sed -i "s|/usr/bin/crossfx|${prefix}/bin/crossfx|g" "${started}"
  fi
done

echo "done. Run 'crossfx', or 'systemctl --user enable --now crossfx' to start it at login."
echo "CrossFX needs a running PipeWire session; it does not replace one."
INNER
chmod 755 "${pkg}/install.sh"

mkdir -p "${root}/dist"
# root-owned entries, so an unpack as root does not hand the tree to whatever uid built it.
tar -C "${stage}" --owner=0 --group=0 --numeric-owner -czf "${root}/dist/${name}.tar.gz" "${name}"
printf '%s\n' "${root}/dist/${name}.tar.gz"
