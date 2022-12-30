// CodeMirror, copyright (c) by Marijn Haverbeke and others
// Distributed under an MIT license: https://codemirror.net/LICENSE

define([
  'codemirror/lib/codemirror',
],
function(CodeMirror) {
  "use strict";
  var GUTTER_ID = "CodeMirror-lint-markers";

  function showTooltip(cm, e, content) {
    var tt = document.createElement("div");
    tt.className = "CodeMirror-l