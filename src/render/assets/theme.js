(function () {
  var button = document.getElementById('theme');
  if (!button) return;
  var root = document.documentElement;
  var prefersDark = window.matchMedia('(prefers-color-scheme: dark)');

  // Untouched, the page is whatever the machine says; the attribute is
  // set only once a reader has asked for the other one. So the current
  // theme is the attribute if there is one and the machine if not.
  function current() {
    return root.dataset.theme || (prefersDark.matches ? 'dark' : 'light');
  }
  // The glyph follows the attribute, in css. All that is left here is to
  // say in words what the icon says in a picture.
  function describe() {
    var now = current();
    button.title =
      'Theme: ' + now + '. Click for ' + (now === 'dark' ? 'light' : 'dark') + '.';
    button.setAttribute('aria-label', button.title);
  }

  describe();
  button.addEventListener('click', function () {
    root.dataset.theme = current() === 'dark' ? 'light' : 'dark';
    describe();
  });
  // The machine can change its mind -- at sunset, on most of them -- and
  // while nothing is pinned the page follows it, so the words must too.
  prefersDark.addEventListener('change', function () {
    if (!root.dataset.theme) describe();
  });
})();
