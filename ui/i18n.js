async function loadLocale(preferred) {
    const lang = (preferred || navigator.language || 'zh-CN').toLowerCase();
    // try exact match then prefix
    const candidates = [lang, lang.split('-')[0], 'zh-cn'];
    for (const c of candidates) {
        try {
            const res = await fetch(`./i18n/${c}.json`);
            if (!res.ok) continue;
            const data = await res.json();
            return data;
        } catch (e) {
            // ignore and try next
        }
    }
    // fallback to zh-CN
    const fallback = await fetch('./i18n/zh-CN.json').then(r => r.json());
    return fallback;
}

function applyTranslations(obj) {
    document.querySelectorAll('[data-i18n]').forEach(el => {
        const key = el.getAttribute('data-i18n');
        const parts = key.split('.');
        let v = obj;
        for (const p of parts) {
            if (v && p in v) v = v[p]; else { v = null; break; }
        }
        if (v !== null && v !== undefined) {
            if (el.tagName === 'INPUT' && el.type === 'button') {
                el.value = v;
            } else if (el.placeholder !== undefined && el.hasAttribute('data-i18n-placeholder')) {
                el.placeholder = v;
            } else {
                el.innerText = v;
            }
        }
    });
}

window.__i18n = { loadLocale, applyTranslations };
