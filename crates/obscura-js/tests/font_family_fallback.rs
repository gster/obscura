use obscura_js::runtime::ObscuraJsRuntime;

fn runtime() -> ObscuraJsRuntime {
    let persona = obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153);
    let mut rt = ObscuraJsRuntime::with_base_url("https://font-family.test/page", persona);
    rt.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    rt.set_url("https://font-family.test/page");
    rt.set_viewport(800.0, 600.0);
    rt.run_page_init();
    rt
}

#[test]
fn canvas_named_font_fallback_preserves_order_tokens_and_real_draws() {
    let mut rt = runtime();
    let failures = rt.evaluate(r###"(() => {
  try {
  const failures = [];
  const check = (name, value) => { if (!value) failures.push(name); };
  const canvas = document.createElement('canvas');
  canvas.width = 240; canvas.height = 80;
  const ctx = canvas.getContext('2d');
  const samples = ['iii', 'WWW', 'AV fi', 'mmmmmmmmmmlli'];
  const generic = ['monospace', 'sans-serif', 'serif'];
  const widths = family => {
    ctx.font = '32px ' + family;
    return samples.map(text => ctx.measureText(text).width);
  };
  const equal = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);
  const bases = Object.fromEntries(generic.map(g => [g, widths(g)]));
  check('real proportional control', bases.serif[1] > bases.serif[0]);
  check('real mono control', Math.abs(bases.monospace[1] - bases.monospace[0]) < 0.001);
  for (const name of ['MissingMonoFamilyQ12', 'MissingSansFamilyQ12', 'MissingTimesFamilyQ12', 'MissingGaramondFamilyQ12', 'MissingCourierFamilyQ12']) {
    for (const g of generic) check('missing ordered fallback: ' + name + '/' + g, equal(widths(JSON.stringify(name) + ', ' + g), bases[g]));
  }
  for (const [name, g] of [['Liberation Mono','monospace'], ['Liberation Serif','serif'], ['Liberation Sans','sans-serif']]) {
    for (const tail of generic) check('actual bundled face: ' + name + '/' + tail, equal(widths(JSON.stringify(name) + ', ' + tail), bases[g]));
  }
  check('ordered real named successor', equal(widths('MissingMonoFamilyQ12, "Liberation Serif", monospace'), bases.serif));
  for (const family of ['"serif", monospace', '"MissingFamilyQ12, serif", monospace', '"Missing  FamilyQ12", monospace', '"Missing\\\"FamilyQ12", monospace']) {
    ctx.font = '11px serif';
    ctx.font = '32px ' + family;
    const serialized = ctx.font;
    check('valid syntax accepted: ' + family, serialized !== '11px serif');
    check('quoted named fallback: ' + family, equal(samples.map(t => ctx.measureText(t).width), bases.monospace));
    ctx.font = '11px serif'; ctx.font = serialized;
    check('serialized family roundtrip: ' + family, equal(samples.map(t => ctx.measureText(t).width), bases.monospace));
  }
  check('empty named family is valid missing fallback', equal(widths('"", serif'), bases.serif));
  check('vendor spelling remains explicit generic compatibility', equal(widths('-apple-system, monospace'), bases['sans-serif']));
  check('quoted vendor spelling is named', equal(widths('"-apple-system", monospace'), bases.monospace));
  for (const name of ['cursive','fantasy','ui-serif','ui-rounded','emoji','math','fangsong']) {
    ctx.font = '11px serif'; ctx.font = '32px ' + name + ', monospace';
    check('valid generic syntax: ' + name, ctx.font !== '11px serif');
  }
  ctx.font = '32px serif';
  const retained = ctx.font;
  ctx.font = '32px serif,';
  check('invalid trailing comma retains', ctx.font === retained);
  for (const invalid of ['32px serif)', '32px serif]', '32px serif; monospace', '32px serif {}']) {
    ctx.font = '19px monospace';
    const before = ctx.font;
    const widthBefore = ctx.measureText('iiiWWW').width;
    ctx.font = invalid;
    check('invalid trailing syntax retains: ' + invalid, ctx.font === before);
    check('invalid trailing syntax keeps selected run: ' + invalid, ctx.measureText('iiiWWW').width === widthBefore);
  }
  const draw = family => {
    ctx.clearRect(0, 0, 240, 80);
    ctx.font = '32px ' + family;
    ctx.fillStyle = '#235981'; ctx.fillText('iiiWWW', 3, 48);
    return Array.from(ctx.getImageData(0, 0, 240, 80).data);
  };
  check('draw uses same fallback as measure', equal(draw('MissingMonoFamilyQ12, serif'), draw('serif')));
  return failures;
  } catch (error) { return {error: error.name, message: error.message, stack: error.stack}; }
})()
"###).unwrap();
    assert_eq!(failures, serde_json::json!([]));
}

#[cfg(feature = "render")]
#[test]
fn connected_dom_font_stacks_follow_missing_names_and_quoted_family_tokens() {
    let mut rt = runtime();
    let failures = rt.evaluate(r#"(() => {
        const failures = [];
        const width = (family, shorthand) => {
            const span = document.createElement('span');
            span.textContent = 'iiiWWW';
            span.style.cssText = 'display:inline-block;' + (shorthand ? 'font:32px ' : 'font-size:32px;font-family:') + family;
            document.body.appendChild(span);
            const result = span.getBoundingClientRect().width;
            span.remove();
            return result;
        };
        for (const shorthand of [false, true]) {
            for (const generic of ['serif', 'sans-serif', 'monospace']) {
                const baseline = width(generic, shorthand);
                if (!(baseline > 0)) failures.push('nonzero connected geometry');
                for (const name of ['MissingMonoFamilyQ12', 'MissingSansFamilyQ12', 'MissingTimesFamilyQ12']) {
                    if (width(JSON.stringify(name) + ', ' + generic, shorthand) !== baseline) failures.push(name + '/' + generic + '/' + shorthand);
                }
            }
            const mono = width('monospace', shorthand);
            for (const family of ['"serif", monospace', '"Absent, serif", monospace']) {
                if (width(family, shorthand) !== mono) failures.push('quoted: ' + family + '/' + shorthand);
            }
        }
        return failures;
    })()"#).unwrap();
    assert_eq!(failures, serde_json::json!([]));
}
