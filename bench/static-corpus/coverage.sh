#!/usr/bin/env bash
# Cross-reference SPEC_REF annotations in the static corpus with section
# headings in docs/spec/ja/language-spec.md.
# Informational only — exits 0 regardless of coverage level.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SPEC="$REPO_ROOT/docs/spec/ja/language-spec.md"

# ── 1. Build ref list: "section_id\tfilename" ─────────────────────────────────
# § (U+00A7) is 2-byte UTF-8; — (U+2014, em dash) is 3-byte UTF-8.
# Strategy: strip "path:linenum:", take the SPEC_REF fragment, keep only the
# part before the "—" description separator, then extract digit runs from that.
# This avoids picking up numbers that appear in the description (e.g. "M11", "2").
tmpfile="$(mktemp)"
trap 'rm -f "$tmpfile"' EXIT

while IFS= read -r fullline; do
  fpath="${fullline%%:*}"
  fname="$(basename "$fpath")"
  tmp="${fullline#*:}"        # strip path:
  rest="${tmp#*:}"            # strip linenum:
  spec_part="${rest#*SPEC_REF:}"  # keep from "SPEC_REF:" onward
  before_dash="${spec_part%%—*}"  # drop "— description" suffix (em dash)
  # Emit one line per section reference; a line may reference multiple (§9.1 / §9.2).
  printf '%s' "$before_dash" | grep -oE '[0-9]+(\.[0-9]+)*' | while IFS= read -r sid; do
    printf '%s\t%s\n' "$sid" "$fname"
  done
done < <(grep -rn "SPEC_REF:" "$SCRIPT_DIR" --include='*.ty') > "$tmpfile"

# ── 2. Cross-reference with spec headings; print report ───────────────────────
awk -v reffile="$tmpfile" '
BEGIN {
  while ((getline line < reffile) > 0) {
    n = split(line, parts, "\t")
    if (n < 2) continue
    sid = parts[1]; fname = parts[2]
    if (ref_files[sid] == "")
      ref_files[sid] = fname
    else
      ref_files[sid] = ref_files[sid] " " fname
  }
  close(reffile)
}

/^## [0-9]+\./ {
  id = $2; sub(/\.$/, "", id)
  title = $0; sub(/^## [0-9]+\. */, "", title)
  spec_ids[id] = 1; spec_titles[id] = title; spec_order[++n_spec] = id
}
/^### [0-9]+\.[0-9]+ / {
  id = $2
  title = $0; sub(/^### [0-9]+\.[0-9]+ */, "", title)
  spec_ids[id] = 1; spec_titles[id] = title; spec_order[++n_spec] = id
}
/^#### [0-9]+\.[0-9]+\.[0-9]+ / {
  id = $2
  title = $0; sub(/^#### [0-9]+\.[0-9]+\.[0-9]+ */, "", title)
  spec_ids[id] = 1; spec_titles[id] = title; spec_order[++n_spec] = id
}

END {
  covered_count = 0
  print "Spec coverage report"
  print "------------------------------------------------------------"
  for (i = 1; i <= n_spec; i++) {
    sid = spec_order[i]
    title = substr(spec_titles[sid], 1, 38)
    if (sid in ref_files) {
      printf "OK  sec %-8s %-38s  %s\n", sid, title, ref_files[sid]
      covered_count++
    } else {
      printf "--  sec %-8s %-38s  (uncovered)\n", sid, title
    }
  }
  print "------------------------------------------------------------"
  pct = (n_spec > 0) ? int(covered_count * 100 / n_spec) : 0
  printf "Summary: %d/%d sections covered (%d%%).\n", covered_count, n_spec, pct

  n_unknown = 0
  for (sid in ref_files) {
    if (!(sid in spec_ids))
      unknown_ids[n_unknown++] = sid
  }
  if (n_unknown > 0) {
    print ""
    print "Unknown SPEC_REF targets (not present in spec headings):"
    for (i = 0; i < n_unknown; i++) {
      sid = unknown_ids[i]
      printf "  sec %-12s  from: %s\n", sid, ref_files[sid]
    }
  }
}
' "$SPEC"

exit 0
