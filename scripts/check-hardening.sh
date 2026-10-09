#!/bin/bash
# Checks the hardening of the program Telamon Launcher ships
# (docs/SECURITY.md, "Build hardening"): what Fedora's build flags
# (%{build_cflags}, %{build_cxxflags}, %{build_ldflags}) are meant to give them,
# read back from the finished ELF files with readelf, so a change of flags, a
# macro that isn't expanded or a build that bypasses them fails the package
# build instead of shipping an unhardened program.
#
#   scripts/check-hardening.sh [--lib] [--cxx] [--require-cet] [--forbid TEXT]... <elf>...
#
# Every file must be
#   - a position-independent executable (ET_DYN with an interpreter), or with
#     --lib a shared object (ET_DYN; the launcher ships none, the option is the
#     framework's and its test covers it);
#   - fully RELRO (a GNU_RELRO segment and BIND_NOW), so the GOT is read-only
#     once the program starts;
#   - non-executable stack (GNU_STACK without E);
#   - free of RPATH and RUNPATH (no library search path from outside the
#     system's);
#   - free of text relocations (TEXTREL).
# Reported, not required: on x86_64, whether the file is marked for Intel CET
# (IBT and SHSTK in the GNU property note); --require-cet makes a missing mark
# an error (it is for files without Rust code, which has no stable switch for
# IBT).
# With --cxx (C++ code) also
#   - built with stack protectors (it calls __stack_chk_fail);
#   - with --forbid TEXT (repeatable): not holding TEXT, as ASCII or UTF-16,
#     for strings only a development or test build may have.
set -euo pipefail

cxx=0
lib=0
require_cet=0
forbid=()
usage() {
    echo "usage: $0 [--lib] [--cxx] [--require-cet] [--forbid TEXT]... <elf>..." >&2
    exit 2
}
while [ $# -gt 0 ]; do
    case $1 in
        --cxx) cxx=1; shift ;;
        --lib) lib=1; shift ;;
        --require-cet) require_cet=1; shift ;;
        --forbid) [ $# -ge 2 ] || usage; forbid+=("$2"); shift 2 ;;
        --) shift; break ;;
        -*) usage ;;
        *) break ;;
    esac
done
[ $# -gt 0 ] || usage
for tool in readelf grep; do
    command -v "$tool" >/dev/null || { echo "check-hardening: $tool is needed" >&2; exit 2; }
done

failed=0
bad() {
    echo "check-hardening: $1: $2" >&2
    failed=1
}

for f in "$@"; do
    [ -f "$f" ] || { bad "$f" "not a file"; continue; }
    header=$(readelf -hW "$f" 2>/dev/null) || { bad "$f" "not an ELF file"; continue; }
    segments=$(readelf -lW "$f")
    dynamic=$(readelf -dW "$f" 2>/dev/null || true)

    if [ "$lib" = 1 ]; then
        grep -Eq '^ *Type: +DYN' <<<"$header" || bad "$f" "not a shared object (Type is not DYN)"
    else
        grep -Eq '^ *Type: +DYN' <<<"$header" || bad "$f" "not a position-independent executable (Type is not DYN)"
        # A DYN file with no interpreter is a shared library, not an executable.
        grep -q 'Requesting program interpreter' <<<"$segments" || bad "$f" "no program interpreter (not a PIE executable)"
    fi
    grep -q 'GNU_RELRO' <<<"$segments" || bad "$f" "no GNU_RELRO segment (partial or no RELRO)"
    if ! grep -Eq '\(BIND_NOW\)|FLAGS.*BIND_NOW|FLAGS_1.*NOW' <<<"$dynamic"; then
        bad "$f" "lazy binding (no BIND_NOW): the GOT stays writable"
    fi
    stack=$(grep 'GNU_STACK' <<<"$segments" || true)
    if [ -z "$stack" ]; then
        bad "$f" "no GNU_STACK segment (the stack may be executable)"
    elif grep -Eq 'GNU_STACK.* RWE |GNU_STACK.* R E ' <<<"$stack"; then
        bad "$f" "executable stack"
    fi
    if grep -Eq '\((RPATH|RUNPATH)\)' <<<"$dynamic"; then
        bad "$f" "has an RPATH or RUNPATH"
    fi
    if grep -Eq '\(TEXTREL\)|FLAGS.*TEXTREL' <<<"$dynamic"; then
        bad "$f" "has text relocations"
    fi

    machine=$(awk -F: '/Machine:/ { gsub(/^ +| +$/, "", $2); print $2 }' <<<"$header")
    case "$machine" in
        *X86-64*)
            notes=$(readelf -nW "$f" 2>/dev/null || true)
            missing=""
            grep -q 'x86 feature:.*IBT' <<<"$notes" || missing="IBT"
            grep -q 'x86 feature:.*SHSTK' <<<"$notes" || missing="${missing:+$missing and }SHSTK"
            if [ -n "$missing" ]; then
                if [ "$require_cet" = 1 ]; then
                    bad "$f" "not marked for Intel CET: $missing (-fcf-protection)"
                else
                    echo "check-hardening: note: $f is not marked for Intel CET: $missing" >&2
                fi
            fi
            ;;
    esac

    if [ "$cxx" = 1 ]; then
        symbols=$(readelf -sW --dyn-syms "$f" 2>/dev/null; readelf -sW "$f" 2>/dev/null || true)
        grep -q '__stack_chk_fail' <<<"$symbols" || bad "$f" "no stack protector (__stack_chk_fail is not used)"
        for text in "${forbid[@]}"; do
            # As ASCII (qgetenv) or as UTF-16 (a QString literal).
            utf16=$(printf '%s' "$text" | sed 's/./&.?/g')
            if grep -aqF -- "$text" "$f" || LC_ALL=C grep -aqE -- "$utf16" "$f"; then
                bad "$f" "holds $text, which only development or test builds may"
            fi
        done
    fi
done

if [ "$failed" = 0 ]; then
    echo "check-hardening: $# file(s) ok"
fi
exit "$failed"
