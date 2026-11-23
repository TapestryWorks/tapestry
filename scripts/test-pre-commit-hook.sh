#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/repo/.githooks" "$tmp/repo/scripts" "$tmp/repo/src" "$tmp/bin"
cp "$ROOT/.githooks/pre-commit" "$tmp/repo/.githooks/pre-commit"
cp "$ROOT/scripts/install-hooks.sh" "$tmp/repo/scripts/install-hooks.sh"

cat > "$tmp/bin/cargo" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$*" == 'fmt --all' ]]; then
  printf '\n// formatted\n' >> src/a.rs
  if [[ "${MOCK_EXTRA:-}" == "yes" ]]; then
    printf '\n// unrelated formatting\n' >> src/b.rs
  fi
elif [[ "$*" == 'fmt --all -- --check' ]]; then
  grep -q '// formatted' src/a.rs
else
  exit 2
fi
MOCK
chmod +x "$tmp/bin/cargo"
export PATH="$tmp/bin:$PATH"

cd "$tmp/repo"
git init -q
git config user.name 'Hook Test'
git config user.email 'hook@example.test'
printf 'fn main() {}\n' > src/a.rs
printf 'fn unused() {}\n' > src/b.rs
git add .
git -c core.hooksPath=/dev/null commit -qm initial
bash scripts/install-hooks.sh

# Successful commit formats and re-stages previously staged Rust content.
printf '// change\n' >> src/a.rs
git add src/a.rs
git commit -qm 'format automatically'
git show HEAD:src/a.rs | grep -q '// formatted'

# Unstaged Rust edits must never be added to a commit implicitly.
printf '// staged\n' >> src/a.rs
git add src/a.rs
printf '// unstaged\n' >> src/a.rs
if git commit -qm 'must fail on unstaged code'; then
  echo 'ERROR: committed unstaged Rust changes' >&2
  exit 1
fi
git show :src/a.rs | grep -qv '// unstaged'
git restore src/a.rs
git reset -q

# If cargo fmt changes an unrelated file, stop rather than stage it.
printf '// staged again\n' >> src/a.rs
git add src/a.rs
if MOCK_EXTRA=yes git commit -qm 'must fail on unrelated formatted file'; then
  echo 'ERROR: committed unrelated formatting' >&2
  exit 1
fi
git diff --cached --name-only | grep -qx 'src/a.rs'
if git diff --cached --name-only | grep -qx 'src/b.rs'; then
  echo 'ERROR: unrelated formatting was staged' >&2
  exit 1
fi

echo 'All pre-commit hook tests passed.'
