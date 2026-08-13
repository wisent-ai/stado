#!/bin/sh
set -eu
rm -rf "$HOME/.stado/build-native-stado"
rm -f \
  "$HOME/.stado/stado-native-source.tar.gz" \
  "$HOME/.stado/bin/stado.next" \
  "$HOME/.stado/bin/build-native-stado"
printf '%s\n' "removed Stado native build inputs"
