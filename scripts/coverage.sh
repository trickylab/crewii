#!/bin/sh
set -eu
cargo llvm-cov --fail-under-lines 95 --ignore-filename-regex 'src/main\.rs' "$@"
node --test --experimental-test-coverage --test-coverage-lines=95 --test-coverage-functions=95 \
  --test-coverage-include='assets/plugin/*.js' 'plugin-tests/*.test.js'
