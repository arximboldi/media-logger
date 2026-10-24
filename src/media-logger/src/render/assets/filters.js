(function () {
  var boxes = Array.prototype.slice.call(
    document.querySelectorAll('#filters input[type=checkbox]'));
  var days = Array.prototype.slice.call(document.querySelectorAll('.cal-day'));
  var bars = Array.prototype.slice.call(document.querySelectorAll('.bar'));
  var rows = Array.prototype.slice.call(document.querySelectorAll('tbody tr[data-host]'));
  var clear = document.getElementById('clear');
  var shown = document.getElementById('shown');
  var qtotal = document.getElementById('qtotal');
  var ptotal = document.getElementById('ptotal');
  if (!bars.length && !days.length) return;
  var dates = [];
  var volpick = document.getElementById('volpick');
  var volrange = document.getElementById('volrange');
  var volnum = document.getElementById('volnum');
  var volmin = 0;

  function short(v) {
    return v.length > 22 ? v.slice(0, 21) + '\u2026' : v;
  }
  // The twin of `human_secs` in render.rs. The page prints the
  // quiet-hours total unfiltered and this rewrites it as the table
  // narrows, so the two must agree to the character: a figure that
  // changed format under a filter would read as a different measurement.
  function humanSecs(secs) {
    function pad(n) { return (n < 10 ? '0' : '') + n; }
    var h = Math.floor(secs / 3600),
        m = Math.floor((secs % 3600) / 60),
        s = secs % 60;
    if (h >= 48) return Math.floor(h / 24) + 'd' + pad(h % 24) + 'h';
    if (h > 0) return h + 'h' + pad(m) + 'm';
    if (m > 0) return m + 'm' + pad(s) + 's';
    return s + 's';
  }
  // The colon belongs to the pill's name, so it is written apart from the
  // value and stays out of the weight the value takes.
  function setCount(group, label) {
    var count = group.querySelector('.count');
    count.textContent = ': ';
    var val = document.createElement('span');
    val.className = 'val';
    val.textContent = label;
    count.appendChild(val);
  }
  function wanted(kind) {
    var all = boxes.filter(function (b) { return b.dataset.kind === kind; });
    if (!all.length) return null;
    var on = all.filter(function (b) { return b.checked; }).map(function (b) { return b.value; });
    return on.length === all.length ? null : on;
  }
  // Loud enough, as set on the slider.
  function loudEnough(el) {
    if (!volmin) return true;
    var v = parseFloat(el.dataset.vol);
    // Nothing was measured here, so nothing can be said about whether it
    // cleared the threshold. Not hiding it: dropping evidence for want of
    // a measurement is the one thing this report must not do quietly.
    return isNaN(v) || v * 100 >= volmin;
  }
  // What kind of thing is shown, as chosen in the dropdowns.
  function matchesDimensions(el) {
    return loudEnough(el) && ['host', 'player', 'device', 'during'].every(function (k) {
      var want = wanted(k);
      return !want || want.indexOf(el.dataset[k]) >= 0;
    });
  }
  // Which day, as chosen in the calendar. Kept separate: picking a day is
  // navigation, and recolouring the calendar by it would blank every other
  // cell and destroy the overview being navigated with.
  function matchesDate(el) {
    return !dates.length || dates.indexOf(el.dataset.date) >= 0;
  }
  function keep(el) {
    return matchesDimensions(el) && matchesDate(el);
  }
  // The calendar must say the same thing as the rows below it, so it is
  // recoloured from whichever rows survive rather than left showing the
  // unfiltered day.
  var months = document.querySelector('.months');
  var scale = months && {
    lo: parseFloat(months.dataset.lo),
    hi: parseFloat(months.dataset.hi),
    // Kept so an unnarrowed view can return to the scale the page was
    // built with rather than to whatever the last filter left behind.
    baseLo: parseFloat(months.dataset.lo),
    baseHi: parseFloat(months.dataset.hi),
    steps: parseInt(months.dataset.steps, 10),
    auto: months.dataset.auto === '1',
    floor: parseFloat(months.dataset.floor)
  };
  var range = document.querySelector('.ramp-range');
  function recolour() {
    if (!scale) return;
    var best = {}, peak = {}, played = {}, quiet = {};
    rows.forEach(function (r) {
      // Dimensions only: the day selection must not recolour the calendar.
      if (!matchesDimensions(r)) return;
      var d = r.dataset.date;
      // That something played is its own fact, kept apart from any level.
      // A level can be missing where playback is not: nothing shorter than
      // five minutes holds one, and a day of short tracks has none at all.
      played[d] = true;
      var v = parseFloat(r.dataset.sustained);
      if (!isNaN(v)) best[d] = Math.max(best[d] === undefined ? -Infinity : best[d], v);
      var p = parseFloat(r.dataset.vol);
      if (!isNaN(p)) peak[d] = Math.max(peak[d] === undefined ? -Infinity : peak[d], p);
      if (parseInt(r.dataset.quiet, 10) > 0) quiet[d] = true;
    });
    // Nothing is narrowed: the page keeps the levels it was built with.
    // Rebuilding them from the rows gives a different and worse number --
    // a day's level is held across whatever was playing, and no single
    // span need hold it, so a day of short tracks came out at the level
    // of its one long one. Only a filter makes rebuilding necessary, and
    // then it is the best that can be had without the samples.
    var narrowed = volmin > 0 || boxes.some(function (b) { return !b.checked; });
    var level = {};
    days.forEach(function (cell) {
      var d = cell.dataset.date;
      if (!narrowed) {
        var own = parseFloat(cell.dataset.level);
        if (!isNaN(own)) { level[d] = own; return; }
      }
      // The level held for five minutes where there is one; the loudest
      // reached where there is not. A day of short tracks holds no level
      // and is still a day that played, and shading it by what it reached
      // says less than the ramp promises but nothing that is untrue.
      if (best[d] !== undefined) level[d] = best[d];
      else if (peak[d] !== undefined) level[d] = peak[d];
    });
    // An automatic scale is taken from what survived, not from what the
    // report was built with: filtering down to one quiet host should spend
    // the ramp on that host's days rather than leave them all at one end.
    // Unnarrowed it is left alone, so the page agrees with the figure
    // printed beside it and with the same report on paper.
    if (scale.auto && narrowed) {
      var lo = Infinity, hi = -Infinity;
      for (var k in level) { lo = Math.min(lo, level[k]); hi = Math.max(hi, level[k]); }
      if (isFinite(lo)) {
        if (hi - lo < scale.floor) {
          lo = Math.max(0, (lo + hi) / 2 - scale.floor / 2);
          hi = lo + scale.floor;
        }
        scale.lo = lo;
        scale.hi = hi;
      }
    } else {
      scale.lo = scale.baseLo;
      scale.hi = scale.baseHi;
    }
    if (range) range.textContent =
      (scale.lo * 100).toFixed(1) + '% to ' + (scale.hi * 100).toFixed(1) + '%';
    days.forEach(function (cell) {
      var d = cell.dataset.date;
      var v = level[d];
      var dot = cell.querySelector('.cal-dot');
      if (!played[d]) {
        // Cleared, not painted: the mark a cell carries is in CSS, and an
        // inline background would cover it.
        cell.style.background = '';
        cell.style.color = '';
        // A day filtered down to nothing is not a day nobody listened to.
        // The daemon was running and what it heard is simply not on show,
        // so the cell says nothing played rather than going blank.
        cell.classList.toggle('cal-silent', cell.dataset.rec !== 'none');
      } else if (v === undefined) {
        // It played, and nothing about it can be measured. Saying nothing
        // is the only honest thing left; saying "nothing played" would be
        // a statement about the room, and this is a gap in the record.
        cell.style.background = '';
        cell.style.color = '';
        cell.classList.remove('cal-silent');
      } else {
        cell.classList.remove('cal-silent');
        var t = (v - scale.lo) / Math.max(1e-9, scale.hi - scale.lo);
        var i = Math.min(scale.steps - 1, Math.max(0, Math.round(t * (scale.steps - 1))));
        cell.style.background = 'var(--ramp-' + i + ')';
        cell.style.color = 'var(--ramp-ink-' + i + ')';
      }
      if (dot) dot.hidden = !quiet[d];
    });
    // And the keys follow the marks, so the legend never explains one that
    // is not on the page or leaves one on it unexplained.
    var sk = document.querySelector('[data-key="silent"]');
    if (sk) sk.hidden = !document.querySelector('.cal-silent');
  }

  // ------------------------------------------------------ block relayout
  //
  // A block is a drawing, not a fact: marks with no readable gap between
  // them share one rounded box because at day scale their halos would
  // otherwise close over each other. Which marks those are depends on
  // which marks are on the page, so a filter changes the answer -- and
  // the blocks were drawn once, in Rust, from every mark. Narrowing to
  // one player left its bars sitting inside a box still outlining the
  // hours the other players had filled.
  //
  // So the blocks are laid again here, over whichever marks survive. Each
  // bar carries the mark it came from (`data-ml`, `data-mw`, in track
  // percent) and the page carries the two drawing distances, so this is
  // the same arithmetic on a smaller set rather than a second idea of
  // what a block is. Filtering only ever removes marks, which only ever
  // widens the gaps between them, so a block can split or shrink but two
  // blocks can never become one -- which is what lets each run be relaid
  // from its own bars alone.
  var tl = document.getElementById('tl');
  var APART = tl ? parseFloat(tl.dataset.apart) : 0;
  var FLOOR = tl ? parseFloat(tl.dataset.floor) : 0;
  var SPACING = tl ? parseFloat(tl.dataset.spacing) : 0;
  // The page as Rust drew it, kept so an unnarrowed view returns to it
  // exactly rather than to this script's reconstruction of it.
  var runs = [];
  if (tl) {
    tl.querySelectorAll('.run').forEach(function (run) {
      var bars = [];
      run.querySelectorAll(':scope > .bar').forEach(function (bar) {
        bars.push({
          el: bar,
          style: bar.getAttribute('style'),
          l: parseFloat(bar.dataset.ml),
          w: parseFloat(bar.dataset.mw)
        });
      });
      // The boundaries as served, detached rather than hidden while the
      // block is relaid: left in place they would sit where a layout
      // that no longer holds had put them.
      var cuts = [];
      run.querySelectorAll(':scope > i').forEach(function (i) { cuts.push(i); });
      // A cut takes the colour of whatever it reveals, which over these
      // hours is the quiet band rather than the track.
      var bands = [];
      run.parentNode.querySelectorAll(':scope > .band').forEach(function (b) {
        var from = parseFloat(b.style.left);
        bands.push([from, from + parseFloat(b.style.width)]);
      });
      runs.push({
        el: run,
        style: run.getAttribute('style'),
        bars: bars,
        cuts: cuts,
        bands: bands,
        extra: [],
        drawn: []
      });
    });
  }

  function relayout(narrowed) {
    runs.forEach(function (run) {
      // Clones and boundaries from the last pass go first, so a run that
      // split and then stopped splitting leaves behind neither an empty
      // box nor a mark at a boundary that has moved.
      run.extra.forEach(function (el) { el.remove(); });
      run.extra = [];
      run.drawn.forEach(function (el) { el.remove(); });
      run.drawn = [];
      run.bars.forEach(function (b) {
        if (b.el.parentNode !== run.el) run.el.appendChild(b.el);
      });
      // Unnarrowed, the markup is already right: put back exactly what
      // the page was served rather than recomputing it.
      if (!narrowed) {
        run.el.setAttribute('style', run.style);
        run.bars.forEach(function (b) { b.el.setAttribute('style', b.style); });
        run.cuts.forEach(function (i) { run.el.appendChild(i); });
        run.el.hidden = false;
        run.el.classList.remove('relaid');
        return;
      }
      run.cuts.forEach(function (i) { i.remove(); });
      var kept = run.bars.filter(function (b) { return !b.el.hidden; });
      run.el.hidden = !kept.length;
      if (!kept.length) return;
      // The twin of `blocks` in render.rs, and the laying loop under it
      // is the twin of the one that writes these bars in the first
      // place. A block closes as soon as a mark opens clear of the
      // furthest edge reached so far -- which is not the last mark's own
      // when two players overlap. `marks_share_a_block_when_the_page_
      // would_collide_them` pins the Rust side; the agreement between
      // the two is what an unnarrowed page returning to its served
      // markup, byte for byte, is checking.
      var groups = [];
      kept.forEach(function (b) {
        var g = groups[groups.length - 1];
        if (g && b.l < g.right + APART) {
          g.bars.push(b);
          g.right = Math.max(g.right, b.l + b.w);
        } else {
          groups.push({ left: b.l, right: b.l + b.w, bars: [b] });
        }
      });
      groups.forEach(function (g, i) {
        // The first group keeps the run; a split borrows an empty copy of
        // it, which carries the class and the shadow but none of the
        // boundary marks, whose adjacencies no longer hold.
        var box = run.el;
        if (i > 0) {
          box = run.el.cloneNode(false);
          box.hidden = false;
          run.el.parentNode.appendChild(box);
          run.extra.push(box);
        }
        box.classList.add('relaid');
        // Laid edge to edge over what is left, each bar running to where
        // the next surviving one began -- so a bar no longer stretches
        // into the time of a neighbour that is no longer shown. Laid
        // before the box is sized, for the reason Rust lays it first:
        // FLOOR can push the last bars past where the marks alone end.
        var cursor = g.left, laid = [];
        g.bars.forEach(function (b, j) {
          var a = Math.max(b.l, cursor);
          var next = g.bars[j + 1];
          var own = b.l + b.w;
          var e = (next && next.l >= own) ? next.l : own;
          e = Math.max(e, a + FLOOR);
          cursor = e;
          laid.push([a, e]);
        });
        var full = Math.max(g.right, cursor) - g.left;
        box.setAttribute('style', 'left:' + g.left.toFixed(3) + '%;width:' + full.toFixed(3) + '%');
        g.bars.forEach(function (b, j) {
          if (b.el.parentNode !== box) box.appendChild(b.el);
          b.el.setAttribute(
            'style',
            b.style.replace(
              /left:[^;]*;width:[^;]*;/,
              'left:' + ((laid[j][0] - g.left) / full * 100).toFixed(3) + '%;' +
              'width:calc(' + ((laid[j][1] - laid[j][0]) / full * 100).toFixed(3) + '% + 1px);'
            )
          );
        });
        // And the boundaries between them, by the rule that drew them in
        // the first place: every one inside the block, marked the same
        // way. They were hidden here at first, on the reasoning that a
        // filter breaks the adjacency a cut stands for. It does not --
        // the bars are still laid edge to edge, so the boundary is still
        // there -- and an album narrowed to one player lost the lines
        // between its songs and read as one long thing playing.
        //
        // Drawn after the bars, so they sit above them however the bars
        // were painted.
        var marked = g.left;
        laid.forEach(function (ab, j) {
          var a = ab[0];
          if (j === 0 || a - marked < SPACING || g.left + full - a < SPACING) return;
          var under = run.bands.some(function (q) { return a >= q[0] && a < q[1]; })
            ? '--quiet' : '--panel';
          var cut = document.createElement('i');
          cut.setAttribute(
            'style',
            'left:' + ((a - g.left) / full * 100).toFixed(3) + '%;background:var(' + under + ')'
          );
          box.appendChild(cut);
          run.drawn.push(cut);
          marked = a;
        });
      });
    });
  }

  function apply() {
    var visible = 0;
    var filtered = dates.length > 0 || volmin > 0 ||
      boxes.some(function (b) { return !b.checked; });
    bars.forEach(function (b) { b.hidden = !keep(b); });
    rows.forEach(function (r) { r.hidden = !keep(r); if (!r.hidden) visible++; });
    relayout(filtered);
    // A timeline row with nothing left in it is noise, not evidence.
    document.querySelectorAll('.row').forEach(function (row) {
      var any = row.querySelector('.bar:not([hidden])');
      if (row.querySelector('.bar')) row.hidden = !any;
    });
    // A day's heading in the table stands over its own rows; with all of
    // them filtered out it stands over the next day's.
    document.querySelectorAll('tr.daybreak').forEach(function (head) {
      var any = false;
      for (var el = head.nextElementSibling; el; el = el.nextElementSibling) {
        if (el.classList.contains('daybreak')) break;
        if (!el.hidden) { any = true; break; }
      }
      head.hidden = !any;
    });
    // A day is drawn as one block and its first row carries the date, so a
    // filter that empties either end has to hand both to the rows left.
    var opened = {}, closes = {};
    document.querySelectorAll('.row[data-day]').forEach(function (row) {
      var when = row.querySelector('.when');
      var day = row.dataset.day;
      row.classList.remove('dayfirst', 'daylast');
      if (row.hidden) { if (when) when.classList.add('ditto'); return; }
      if (opened[day]) { if (when) when.classList.add('ditto'); }
      else {
        row.classList.add('dayfirst');
        if (when) when.classList.remove('ditto');
        opened[day] = true;
      }
      closes[day] = row;
    });
    Object.keys(closes).forEach(function (day) {
      closes[day].classList.add('daylast');
    });
    // A month heading standing over no rows is worse than noise: it says a
    // month is covered while showing nothing from it.
    document.querySelectorAll('.tl-month').forEach(function (head) {
      var axis = head.nextElementSibling;
      var used = false;
      for (var el = axis; el; el = el.nextElementSibling) {
        if (el.classList.contains('tl-month')) break;
        if (el.classList.contains('row') && !el.hidden) { used = true; break; }
      }
      head.hidden = !used;
      if (axis && axis.classList.contains('axis')) axis.hidden = !used;
    });
    days.forEach(function (d) {
      d.setAttribute('aria-pressed', dates.indexOf(d.dataset.date) >= 0 ? 'true' : 'false');
    });
    document.querySelectorAll('.pick:not(.vol)').forEach(function (group) {
      var all = Array.prototype.slice.call(group.querySelectorAll('input'));
      var on = all.filter(function (x) { return x.checked; });
      var label;
      // Name the selection when naming it is short enough to read; fall
      // back to a count only when it is not.
      if (!on.length) label = 'none';
      else if (on.length === all.length) label = 'all';
      else if (on.length === 1) label = short(on[0].value);
      else label = on.length + ' of ' + all.length;
      setCount(group, label);
      // A dimension standing at all or at none is not choosing between its
      // values; one that has picked some of them is, and that is the state
      // worth finding at a glance across a row of four.
      // Anything but "all" is a choice the reader made, none included:
      // showing nothing is as much a filter as showing two of three.
      var custom = on.length < all.length;
      group.classList.toggle('narrowed', custom);
      // And in the list itself, so a dropdown says what is picked without
      // the reader having to read the boxes -- except where everything is
      // picked, since a list with every line in bold says no more than a
      // list with none.
      all.forEach(function (x) {
        if (x.parentNode) x.parentNode.classList.toggle('on', x.checked && custom);
      });
    });
    if (volpick) {
      setCount(volpick, volmin ? '\u2265 ' + volmin + '%' : 'all');
      volpick.classList.toggle('narrowed', volmin > 0);
    }
    recolour();
    clear.hidden = !filtered;
    shown.textContent = filtered ? visible + ' of ' + rows.length + ' shown' : '';
    // The quiet-hours figure is a sum over the rows, so it has to follow
    // them: left alone it went on answering for the whole window while
    // the table under it had been narrowed to one player, which is the
    // one number on this page a reader is most likely to quote.
    //
    // Recomputed always, not only when narrowed. Unlike the calendar's
    // level -- which cannot be rebuilt from what survives and so keeps
    // the figure the page was built with -- this one is exact from the
    // rows: every row carries its own seconds and the total is their sum.
    if (qtotal || ptotal) {
      var qsum = 0, psum = 0;
      rows.forEach(function (r) {
        if (r.hidden) return;
        qsum += parseInt(r.dataset.quiet, 10) || 0;
        psum += parseInt(r.dataset.played, 10) || 0;
      });
      if (qtotal) {
        qtotal.textContent = qsum ? humanSecs(qsum) : 'none';
        qtotal.classList.toggle('flag', qsum > 0);
      }
      if (ptotal) ptotal.textContent = humanSecs(psum);
    }
  }

  boxes.forEach(function (b) { b.addEventListener('change', apply); });

  // The slider and the box are two ways of saying one number, so each
  // writes the other rather than keeping a figure of its own.
  function setVol(n) {
    n = Math.round(n);
    volmin = isNaN(n) ? 0 : Math.max(0, Math.min(100, n));
    if (volrange) volrange.value = volmin;
    if (volnum && String(volmin) !== volnum.value) volnum.value = volmin;
    apply();
  }
  if (volrange) {
    volrange.addEventListener('input', function () { setVol(+volrange.value); });
  }
  if (volnum) {
    volnum.addEventListener('input', function () { setVol(+volnum.value); });
  }

  // Shift-click is a selection gesture here, not a text one; without this
  // the browser extends its text selection and drags the labels into it.
  function noTextDrag(e) {
    if (e.shiftKey) e.preventDefault();
  }

  // Clicking a name means "show only this", and again means "show
  // everything" — the question being asked most of the time. The checkbox
  // and shift-click stay available for building a set by hand.
  document.querySelectorAll('.pick').forEach(function (group) {
    var all = Array.prototype.slice.call(group.querySelectorAll('input'));
    group.querySelectorAll('label').forEach(function (lab) {
      lab.addEventListener('mousedown', noTextDrag);
      lab.addEventListener('click', function (e) {
        if (e.target.tagName === 'INPUT') return;   // the box toggles itself
        e.preventDefault();
        var box = lab.querySelector('input');
        if (e.shiftKey) {
          box.checked = !box.checked;
        } else {
          var soloed = box.checked && all.every(function (x) {
            return x === box || !x.checked;
          });
          all.forEach(function (x) { x.checked = soloed || x === box; });
        }
        apply();
      });
    });
  });
  // A details element stays open until its own summary is clicked again,
  // which is wrong for something that behaves like a menu.
  document.addEventListener('click', function (e) {
    document.querySelectorAll('.pick[open]').forEach(function (group) {
      if (!group.contains(e.target)) group.open = false;
    });
  });
  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Escape') return;
    document.querySelectorAll('.pick[open]').forEach(function (g) { g.open = false; });
  });
  function pickDay(cell, extend) {
    var when = cell.dataset.date;
    if (extend) {
      var i = dates.indexOf(when);
      if (i >= 0) dates.splice(i, 1); else dates.push(when);
    } else {
      // Absolute: this day alone, or back to all of them.
      dates = (dates.length === 1 && dates[0] === when) ? [] : [when];
    }
    apply();
  }
  days.forEach(function (d) {
    d.addEventListener('mousedown', noTextDrag);
    d.addEventListener('click', function (e) { pickDay(d, e.shiftKey); });
    // The cells are focusable, so they have to answer the keyboard too.
    d.addEventListener('keydown', function (e) {
      if (e.key !== 'Enter' && e.key !== ' ') return;
      e.preventDefault();
      pickDay(d, e.shiftKey);
    });
  });
  clear.addEventListener('click', function () {
    dates = [];
    boxes.forEach(function (b) { b.checked = true; });
    setVol(0);
  });
  apply();
})();
