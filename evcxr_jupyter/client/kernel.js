// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

define([
    'require',
    'base/js/namespace',
    'codemirror/lib/codemirror',
    'base/js/events',
    './lint.js'
], function (requireJs, Jupyter, CodeMirror, events) {
    "use strict";

    function initCell(cell) {
        // It could be nice to show errors and warnings in the gutter as well.
        // We can enable that with the following line, however unfortunately
        // that messes up the horizontal scroll until the user clicks in editor.
        // We can sort of fix that 