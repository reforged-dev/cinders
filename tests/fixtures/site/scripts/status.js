// A classic script: its top-level names are globals that other scripts may read.
var statusText = "✓ ready";
function showStatus() {
  document.body.dataset.status = statusText;
}
