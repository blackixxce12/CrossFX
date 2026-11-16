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
# (crates/crossfx-app/src/notify.rs) and the sound settings look the icon up by.
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
install -Dm644 crates/crossfx-rnnoise/COPYING "${pkg}/share/doc/crossfx/COPYING.rnnoise"
install -Dm644 assets/fonts/OFL.txt "${pkg}/share/doc/crossfx/OFL.noto"
install -Dm644 assets/fonts/OFL-Manrope.txt "${pkg}/share/doc/crossfx/OFL.manrope"

cat > "${pkg}/install.sh" <<'INNER'
#!/usr/bin/env sh
# Copy this tree into a prefix. Default /usr/local; pass another as the first argument.
set -eu
# What it copies everyone may read and run as the tarball has it, whatever the shell's umask.
umask 022
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

# How to run this script for /usr/local: the way it was run, when that was as root — through
# sudo, doas or pkexec, or as root itself, after su or at a root login — and as root otherwise.
for_usr_local() {
  if [ "$(id -u 2>/dev/null || :)" != 0 ]; then
    echo "run '${here}/install.sh' as root, through sudo, doas or su,"
  elif [ -n "${SUDO_USER:-}" ]; then
    echo "run 'sudo ${here}/install.sh'"
  elif [ -n "${DOAS_USER:-}" ]; then
    echo "run 'doas ${here}/install.sh'"
  elif [ -n "${PKEXEC_UID:-}" ]; then
    echo "run 'pkexec ${here}/install.sh'"
  else
    echo "run '${here}/install.sh'"
  fi
}

# Stop, saying why, before anything has been changed.
refuse() {
  echo "refusing to install into ${prefix}: $* CrossFX finds its factory presets only under" \
    "/usr and /usr/local: $(for_usr_local) to install into /usr/local." >&2
  exit 1
}

# The list of the files an install put in share/crossfx, which it leaves there: the next install
# tells them from a user's by it, and removes those it no longer ships.
manifest=.installed-files

# Refuse the prefix when it, or the folder its share/crossfx leads to, is the folder DIR or lies
# under it; the words after DIR say what DIR is. A home that is / itself (a service account's, an
# emptied environment's) keeps nothing clear: the data home under it is still kept clear by
# not_onto.
not_inside() {
  n_dir="$(resolved "$1")"
  shift
  [ "${n_dir}" != / ] || return 0
  if inside "${target}" "${n_dir}"; then
    refuse "it is inside $*."
  elif inside "${data_target}" "${n_dir}"; then
    refuse "its share/crossfx leads to ${data_target}, inside $*."
  fi
}

# Refuse the prefix when its share folder is DATA, a user's data home.
not_onto() {
  if [ "$(resolved "${target}/share")" = "$(resolved "$1")" ]; then
    refuse "its share folder is $1, a user's data home, where CrossFX keeps that user's own" \
      "presets ($1/crossfx)."
  fi
}

# Lines of the password database: every account it lists, or the one named or numbered $1.
# getent asks every source the system has, a directory service included; /etc/passwd is all
# there is to read where getent is missing.
passwd_lines() {
  if command -v getent >/dev/null 2>&1; then
    getent passwd "$@" 2>/dev/null || :
  elif [ "$#" -eq 0 ]; then
    cat /etc/passwd 2>/dev/null || :
  else
    awk -F: -v key="$1" '$1 == key || $3 == key' /etc/passwd 2>/dev/null || :
  fi
}

# The number /etc/login.defs gives the key $1, or $2 when it gives none.
login_defs() {
  l_value="$(awk -v key="$1" '$1 == key && $2 ~ /^[0-9]+$/ { value = $2 } END { print value }' \
    /etc/login.defs 2>/dev/null)" || l_value=
  printf '%s\n' "${l_value:-$2}"
}

# The uids of people's accounts start at UID_MIN of /etc/login.defs (1000 when it names none) and
# have no end: UID_MAX (60000 by default) only bounds what useradd hands out, and the accounts a
# directory service gives people — SSSD's mapping of Active Directory from 200000 on, FreeIPA's
# ranges above a billion — and systemd-homed's, from 60001, lie above it. Two ids up there are
# nobody's: 65534 and 4294967294, the overflow ids an owner the kernel or NFS cannot map shows as,
# and the uids of nobody and nfsnobody.
uid_min="$(login_defs UID_MIN 1000)"

# Whether the uid $1, a number, is of an account the system gives a person.
person() {
  [ "$1" -ge "${uid_min}" ] && [ "$1" -ne 65534 ] && [ "$1" -ne 4294967294 ]
}

# The first entry under the folder DIR, as find lists LISTED, that is none of: what the tarball's
# folder SHIPPED has of the same name and kind; a file the list INSTALLED of an earlier install
# names, or a folder it names a file in; that list itself. A link is none of them. Nothing when
# every entry is one of them.
unshipped() {
  while IFS= read -r u_entry; do
    [ -n "${u_entry}" ] || continue
    if [ -h "$1/${u_entry}" ]; then
      :
    elif [ -d "$1/${u_entry}" ]; then
      [ -d "$2/${u_entry}" ] && continue
      printf '%s\n' "$4" | U_DIR="${u_entry}/" awk '
        index($0, ENVIRON["U_DIR"]) == 1 { found = 1; exit }
        END { exit !found }' && continue
    elif [ -f "$1/${u_entry}" ]; then
      [ -f "$2/${u_entry}" ] && continue
      [ "${u_entry}" = "./${manifest}" ] && continue
      printf '%s\n' "$4" | grep -Fxq -- "${u_entry}" && continue
    fi
    printf '%s\n' "${u_entry#./}"
    return 0
  done <<UNSHIPPED
$3
UNSHIPPED
}

# Refuse the resolved path PATH, as root, when the deepest folder of it that exists, or one above
# that, belongs to one of the accounts the system gives people (person): a home the password
# database does not list — a directory service's, its enumeration off — still belongs to its
# user. The words after PATH lead up to the folder.
not_owned() {
  o_dir=$1
  shift
  while [ ! -d "${o_dir}" ]; do
    o_dir="$(dirname -- "${o_dir}")"
  done
  while :; do
    o_uid="$(ls -ldn -- "${o_dir}" 2>/dev/null | awk 'NR == 1 { print $3 }')" || o_uid=
    case "${o_uid}" in
      '' | *[!0-9]*) ;;
      *)
        if person "${o_uid}"; then
          o_name="$(passwd_lines "${o_uid}" | head -n 1 | cut -d: -f1)"
          refuse "$* ${o_dir}, which belongs to ${o_name:-uid ${o_uid}}, one of the accounts the" \
            "system gives people: it may be that user's home, where CrossFX keeps their own" \
            "presets."
        fi
        ;;
    esac
    [ "${o_dir}" != / ] || return 0
    o_dir="$(dirname -- "${o_dir}")"
  done
}

case "${prefix}" in
  /usr|/usr/local) ;;
  *)
    # Not into anyone's home folder. CrossFX keeps a user's own presets, voice presets and
    # unsaved edits in their data home's crossfx/presets, which is <prefix>/share/crossfx/presets
    # for a prefix such as ~/.local: the copy below would lay the factory presets over every one
    # of theirs that shares a name — the voice presets a user tuned keep the factory names — and
    # the binary, which looks for factory presets only under /usr and /usr/local, would list none
    # of them as such.
    target="$(resolved "${prefix}")"
    # Where the factory presets land: under the prefix, unless its share folder is a link.
    data_target="$(resolved "${target}/share/crossfx")"
    # The user sudo, doas or pkexec was run by: there $HOME is normally root's, so their home
    # comes from the password database, and one it cannot find is a home it cannot keep clear of.
    invoked=
    for invoker in "${SUDO_USER:-}" "${DOAS_USER:-}" "${PKEXEC_UID:-}"; do
      [ -n "${invoker}" ] || continue
      invoked=1
      entry="$(passwd_lines "${invoker}" | head -n 1)"
      name=${entry%%:*}
      home="$(printf '%s\n' "${entry}" | cut -d: -f6)"
      case "${home}" in
        /*) ;;
        *)
          refuse "it cannot tell where ${invoker}'s home is, where CrossFX keeps that user's own" \
            "presets."
          ;;
      esac
      not_inside "${home}" "${name}'s home, ${home}, where CrossFX keeps that user's own presets" \
        "(${home}/.local/share/crossfx)"
      not_onto "${home%/}/.local/share"
    done
    # Whoever $HOME and $XDG_DATA_HOME belong to: the user running this, or under sudo the one
    # sudo left them to.
    user="${USER:-$(id -un 2>/dev/null || echo this user)}"
    case "${HOME:-}" in
      /*)
        not_inside "${HOME}" "${user}'s home, ${HOME}, where CrossFX keeps that user's own" \
          "presets (${HOME}/.local/share/crossfx)"
        not_onto "${HOME%/}/.local/share"
        ;;
      *)
        if [ -z "${invoked}" ]; then
          refuse "it cannot tell where ${user}'s home is, where CrossFX keeps that user's own" \
            "presets."
        fi
        ;;
    esac
    case "${XDG_DATA_HOME:-}" in
      /*) not_onto "${XDG_DATA_HOME}" ;;
    esac
    # Nor any other account's. Run as root — under su, doas or pkexec, from a root login, or by
    # an administrator's sudo for someone else — the copy reaches every home on the system, and
    # none of those says whose prefix it was given. So the homes of the accounts the system gives
    # people (person, above: the uids from UID_MIN up, a directory service's among them), which
    # leaves out the system accounts whose "homes" are /, /var/www or /usr/sbin; and as root also
    # /home, root's own home, and any folder that belongs to one of those accounts, for the homes
    # a directory service does not list.
    homes="$(passwd_lines | awk -F: '$3 ~ /^[0-9]+$/ && $6 ~ /^\/./ { print $3 ":" $1 ":" $6 }')"
    while IFS=: read -r uid name home; do
      [ -n "${home}" ] && person "${uid}" || continue
      not_inside "${home}" "${name}'s home, ${home}, where CrossFX keeps that user's own presets" \
        "(${home}/.local/share/crossfx)"
    done <<HOMES
${homes}
HOMES
    if [ "$(id -u 2>/dev/null || :)" = 0 ]; then
      not_inside /home "/home, where the users' home folders are"
      not_inside /root "root's home, /root"
      home="$(passwd_lines 0 | head -n 1 | cut -d: -f6)"
      case "${home}" in
        /?*) not_inside "${home}" "root's home, ${home}" ;;
      esac
      not_owned "${target}" "it is in"
      not_owned "${data_target}" "its share/crossfx leads to ${data_target}, in"
    fi
    # And not over a folder of someone's presets, wherever it lies. All a CrossFX tarball puts in
    # share/crossfx is the factory presets, and the list of them it leaves there, so anything
    # else already there — a user's own presets, their unsaved edits in AutoSave/, the .bak of a
    # preset saved over, the fonts they added — is that user's data, which the copy would mix the
    # factory presets into and replace where a name is shared. What an earlier install's list
    # names is that install's own: installing a later tarball over it is an upgrade.
    data="${prefix}/share/crossfx"
    if [ -h "${data}" ] || { [ -e "${data}" ] && [ ! -d "${data}" ]; }; then
      refuse "${data} is not a folder of its own, and the copy would follow it to whatever it" \
        "leads to."
    fi
    if [ -d "${data}" ]; then
      listed="$(cd -- "${data}" && find . ! -name . -print)" ||
        refuse "it cannot read all of ${data}, so it cannot tell whether a user keeps presets" \
          "there."
      installed=
      if [ -f "${data}/${manifest}" ] && [ ! -h "${data}/${manifest}" ]; then
        installed="$(cat -- "${data}/${manifest}")" ||
          refuse "it cannot read ${data}/${manifest}, the list of what CrossFX installed there."
      fi
      stray="$(unshipped "${data}" "${here}/share/crossfx" "${listed}" "${installed}")"
      if [ -n "${stray}" ]; then
        if [ -h "${data}/${stray}" ]; then
          refuse "${data}/${stray} is a link, and the copy would follow it to whatever it leads" \
            "to."
        fi
        refuse "${data} already holds ${stray}, which neither this CrossFX ships nor an earlier" \
          "one installed there: a folder of a user's own presets looks like that."
      fi
    fi
    # Not a refusal, because there are reasons to put a binary elsewhere — but the presets will
    # not be found there: the binary searches exactly /usr/share/crossfx and
    # /usr/local/share/crossfx (install_data_dirs in crates/crossfx-preset/src/lib.rs), not
    # $XDG_DATA_DIRS, and systemd --user does not look for units under an arbitrary prefix either.
    echo "warning: CrossFX only looks for its presets under /usr and /usr/local; installed" >&2
    echo "         under ${prefix} it will start with an empty preset list, and systemd will" >&2
    echo "         not find ${prefix}/lib/systemd/user/crossfx.service." >&2
    ;;
esac

echo "installing CrossFX into ${prefix}"
mkdir -p "${prefix}"
# With the tarball's modes and the owner who runs this: as root, root's, not those of whoever
# unpacked the tarball, as `cp -a` left them — a binary on the path that user could replace, and
# folders a later install as root would take for that user's.
cp -R "${here}/bin" "${here}/share" "${prefix}/"
[ -d "${here}/lib" ] && cp -R "${here}/lib" "${prefix}/"
data="${prefix}/share/crossfx"
# An upgrade: what the earlier install's list names and this tarball no longer ships goes, so that
# a preset an earlier CrossFX shipped and this one dropped is not listed as a factory preset still.
# Only a file of the list's, inside share/crossfx: a line that leaves the folder, a link, or a
# folder of share/crossfx that leads elsewhere is never followed.
if [ -f "${data}/${manifest}" ] && [ ! -h "${data}/${manifest}" ]; then
  while IFS= read -r c_line; do
    case "${c_line}" in
      ./?*) ;;
      *) continue ;;
    esac
    case "/${c_line#./}/" in
      */../* | */./*) continue ;;
    esac
    c_path="${data}/${c_line#./}"
    [ ! -f "${here}/share/crossfx/${c_line}" ] || continue
    [ -f "${c_path}" ] && [ ! -h "${c_path}" ] || continue
    c_dir="$(dirname -- "${c_path}")"
    inside "$(resolved "${c_dir}")" "$(resolved "${data}")" || continue
    rm -f -- "${c_path}"
    # And the folders it leaves empty, up to share/crossfx.
    while [ "${c_dir}" != "${data}" ] && rmdir -- "${c_dir}" 2>/dev/null; do
      c_dir="$(dirname -- "${c_dir}")"
    done
  done <"${data}/${manifest}"
fi
# The list of what it put in share/crossfx, for the next install.
(cd "${here}/share/crossfx" && find . -type f -print) >"${data}/${manifest}"

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
