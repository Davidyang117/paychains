#!/usr/bin/env bash

PERF_LIBS_VERSION=v0.19.3
VERSION=$PERF_LIBS_VERSION-1

set -e
cd "$(dirname "$0")"

if [[ $VERSION != "$(cat target/perf-libs/.version 2> /dev/null)" ]]; then
  if [[ $(uname) != Linux ]]; then
    echo Note: Performance libraries are only available for Linux
    exit 0
  fi

  if [[ $(uname -m) != x86_64 ]]; then
    echo Note: Performance libraries are only available for x86_64 architecture
    exit 0
  fi

  rm -rf target/perf-libs
  mkdir -p target/perf-libs
  (
    set -x
    cd target/perf-libs

    if [[ -r ~/.cache/paychains-perf-$PERF_LIBS_VERSION.tgz ]]; then
      cp ~/.cache/paychains-perf-$PERF_LIBS_VERSION.tgz paychains-perf.tgz
    else
      curl -L --retry 5 --retry-delay 2 --retry-connrefused -o paychains-perf.tgz \
        https://github.com/paychains-labs/paychains-perf-libs/releases/download/$PERF_LIBS_VERSION/paychains-perf.tgz
    fi
    tar zxvf paychains-perf.tgz

    if [[ ! -r ~/.cache/paychains-perf-$PERF_LIBS_VERSION.tgz ]]; then
      # Save it for next time
      mkdir -p ~/.cache
      mv paychains-perf.tgz ~/.cache/paychains-perf-$PERF_LIBS_VERSION.tgz
    fi
    echo "$VERSION" > .version
  )

  # Setup symlinks so the perf-libs/ can be found from all binaries run out of
  # target/
  for dir in target/{debug,release}/{,deps/}; do
    mkdir -p $dir
    ln -sfT ../perf-libs ${dir}perf-libs
  done

fi

exit 0
