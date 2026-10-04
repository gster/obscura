//! Regression for all-unavailable Named stacks through the real Canvas ops.
use obscura_js::runtime::ObscuraJsRuntime;

#[test]
fn unavailable_named_stack_keeps_textmetrics_and_real_fill_stroke() {
    let mut runtime=ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::MacChrome153));
    runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    runtime.set_url("https://canvas.test/geometry");runtime.run_page_init();
    let result=runtime.evaluate(r#"(() => {
        const fields=['width','actualBoundingBoxLeft','actualBoundingBoxRight','actualBoundingBoxAscent',
            'actualBoundingBoxDescent','fontBoundingBoxAscent','fontBoundingBoxDescent'];
        const context=font=>{const c=document.createElement('canvas');c.width=160;c.height=80;
            const ctx=c.getContext('2d');ctx.font=font;return ctx;};
        let samples=0,paints=0;
        for (const prefix of ['', 'bold ', 'italic ', 'italic bold ', '550 ']) {
            const reference=context(prefix+'16px sans-serif');
            for(const stack of ["'Geometry Sole Missing Face'", "'Geometry Missing One', 'Geometry Missing Two'"]) {
                const ctx=context(prefix+'16px '+stack);
                for(const baseline of ['alphabetic','top','middle','bottom']) {
                    ctx.textBaseline=reference.textBaseline=baseline;ctx.textAlign=reference.textAlign='center';
                    for(const text of ['', 'Mg', ' ']) {
                        const a=ctx.measureText(text),b=reference.measureText(text);
                        if(!(a instanceof TextMetrics)||!fields.every(k=>Number.isFinite(a[k])&&a[k]===b[k]))
                            return {error:'metrics',prefix,stack,baseline,text};
                        if(text==='Mg'&&!(a.width>0))return {error:'advance'};
                        samples++;
                    }
                }
                for(const stroke of [false,true]) {
                    const a=context(prefix+'16px '+stack),b=context(prefix+'16px sans-serif');
                    const raster=target=>{target.textBaseline='top';target.fillStyle=target.strokeStyle='#1e5aaa';
                        target.globalAlpha=0.8;target.lineWidth=1.25;
                        target[stroke?'strokeText':'fillText']('Mg j',12.25,18,32);
                        return target.getImageData(0,0,160,80).data;};
                    const ap=raster(a),bp=raster(b);
                    if(!ap.some((v,i)=>i%4===3&&v>0)||!ap.every((v,i)=>v===bp[i]))
                        return {error:'paint',prefix,stack,stroke};
                    paints++;
                }
            }
        }
        return {samples,paints};
    })()"#).unwrap();
    assert_eq!(result,serde_json::json!({"samples":120,"paints":20}));
}
