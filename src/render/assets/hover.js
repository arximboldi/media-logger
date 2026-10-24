(function () {
  var tip = document.getElementById('tip');
  // With the script running, the native title would duplicate the tooltip
  // on a still cursor; without it, the attribute is the whole fallback.
  document.querySelectorAll('.bar[title]').forEach(function (bar) {
    bar.dataset.label = bar.getAttribute('title');
    bar.removeAttribute('title');
  });
  function read(bar, frac) {
    var g = (bar.dataset.g || '').split(',');
    var s0 = +bar.dataset.s0, s1 = +bar.dataset.s1;
    var i = Math.min(g.length - 1, Math.max(0, Math.floor(frac * g.length)));
    var when = new Date((s0 + (s1 - s0) * ((i + 0.5) / g.length)) * 1000);
    var hh = String(when.getHours()).padStart(2, '0');
    var mm = String(when.getMinutes()).padStart(2, '0');
    var gain = g[i] ? ' \u00b7 volume ' + g[i] : '';
    return (bar.dataset.label || '') + '\n' + hh + ':' + mm + gain;
  }
  document.addEventListener('mousemove', function (e) {
    var bar = e.target.closest ? e.target.closest('.bar') : null;
    if (!bar) { tip.style.opacity = 0; return; }
    var box = bar.getBoundingClientRect();
    var parts = read(bar, (e.clientX - box.left) / Math.max(1, box.width)).split('\n');
    tip.textContent = '';
    var b = document.createElement('b'); b.textContent = parts[0];
    var s = document.createElement('i'); s.textContent = parts[1];
    tip.appendChild(b); tip.appendChild(s);
    tip.style.opacity = 1;
    tip.style.left = Math.min(window.innerWidth - tip.offsetWidth - 12, e.clientX + 12) + 'px';
    tip.style.top = Math.max(4, e.clientY - tip.offsetHeight - 10) + 'px';
  });
  document.addEventListener('mouseleave', function () { tip.style.opacity = 0; });
})();
