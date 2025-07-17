#!/usr/bin/env bash
#
# Fetches the latest SPL programs and produces the solana-genesis command-line
# arguments needed to install them
#

set -e

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)

source "$here"/fetch-programs.sh

PREFIX="spl"

programs=()

add_spl_program_to_fetch() {
  declare name=$1
  declare version=$2
  declare address=$3
  declare loader=$4

  so_name="${PREFIX}_${name//-/_}.so"
  download_url="https://github.com/solana-program/$name/releases/download/program@v$version/$so_name"

  programs+=("$name $version $address $loader $download_url")
}

add_spl_program_to_fetch token 3.5.0 Gorbj8Dp27NkXMQUkeHBSmpf6iQ3yT4b2uVe8kM4s6br BPFLoader2111111111111111111111111111111111
add_spl_program_to_fetch token-2022 8.0.0 G22oYgZ6LnVcy7v8eSNi2xpNk1NcZiPD8CVKSTut7oZ6 BPFLoaderUpgradeab1e11111111111111111111111
add_spl_program_to_fetch memo  1.0.0 Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo BPFLoader1111111111111111111111111111111111
add_spl_program_to_fetch memo  3.0.0 MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr BPFLoader2111111111111111111111111111111111
add_spl_program_to_fetch associated-token-account 1.1.2 GoATGVNeSXerFerPqTJ8hcED1msPWHHLxao2vwBYqowm BPFLoader2111111111111111111111111111111111
add_spl_program_to_fetch feature-proposal 1.0.0 Feat1YXHhH6t1juaWF74WLcfv4XoNocjXA6sPWHNgAse BPFLoader2111111111111111111111111111111111

fetch_programs "$PREFIX" "${programs[@]}"
