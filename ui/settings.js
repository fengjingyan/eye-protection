const invoke = window.__TAURI__.invoke;

// 获取DOM元素
const workTimeInput = document.getElementById('workTime');
const restTimeInput = document.getElementById('restTime');
const longWorkThresholdInput = document.getElementById('longWorkThreshold');
const longRestTimeInput = document.getElementById('longRestTime');
const opacitySlider = document.getElementById('opacity');
const opacityValue = document.getElementById('opacityValue');
const languageSelect = document.getElementById('language');
const autoStartCheckbox = document.getElementById('autoStart');
const okBtn = document.getElementById('okBtn');
const applyBtn = document.getElementById('applyBtn');
const cancelBtn = document.getElementById('cancelBtn');

const appWindow = window.__TAURI__.window.appWindow;
const listen = window.__TAURI__.event.listen;

let currentSettings = null;
let currentLocale = null;

// i18n 辅助：按 key 取翻译文本，支持 {varName} 占位符替换
function t(key, vars = {}) {
    if (!currentLocale) return key;
    const parts = key.split('.');
    let v = currentLocale;
    for (const p of parts) {
        if (v && p in v) v = v[p]; else return key;
    }
    if (typeof v !== 'string') return key;
    return v.replace(/\{(\w+)\}/g, (_, k) => String(vars[k] ?? k));
}

function opacityToDisplay(v) {
    return Math.round(parseFloat(v) * 100) + '%';
}

// 校验累计阈值与连续工作时间的关系
// 返回 true 表示通过，false 表示有硬约束错误
function validateTimerSettings() {
    const workTime = parseInt(workTimeInput.value) || 1;
    const threshold = parseInt(longWorkThresholdInput.value) || 0;
    const minThreshold = workTime * 2;

    // 动态更新 min 属性
    longWorkThresholdInput.min = minThreshold;

    const msgEl = document.getElementById('timerValidation');

    if (threshold < minThreshold) {
        // 硬约束：阈值过小，阻止保存
        msgEl.textContent = t('settings.validation.thresholdTooSmall', { min: minThreshold });
        msgEl.className = 'validation-msg validation-error';
        msgEl.hidden = false;
        scheduleResize();
        return false;
    }

    // 软提示：显示将在第几轮触发长时休息
    const fullRounds = Math.floor(threshold / workTime);
    const remainder = threshold % workTime;
    if (remainder === 0) {
        msgEl.textContent = t('settings.validation.thresholdExact', { round: fullRounds });
    } else {
        msgEl.textContent = t('settings.validation.thresholdMid', { round: fullRounds + 1, mins: remainder });
    }
    msgEl.className = 'validation-msg validation-info';
    msgEl.hidden = false;
    scheduleResize();
    return true;
}

// 加载设置
async function loadSettings() {
    try {
        currentSettings = await invoke('get_settings');

        workTimeInput.value = currentSettings.work_time;
        restTimeInput.value = currentSettings.rest_time;
        longWorkThresholdInput.value = currentSettings.long_work_threshold_mins ?? 600;
        longRestTimeInput.value = currentSettings.long_rest_mins ?? 5;
        opacitySlider.value = currentSettings.opacity;
        opacityValue.textContent = opacityToDisplay(currentSettings.opacity);
        languageSelect.value = currentSettings.language ?? 'zh-CN';
        autoStartCheckbox.checked = currentSettings.auto_start;

        // 用已保存的语言加载 locale，并刷新页面翻译
        currentLocale = await window.__i18n.loadLocale(currentSettings.language);
        window.__i18n.applyTranslations(currentLocale);
        validateTimerSettings();
    } catch (e) {
        console.error('Failed to load settings:', e);
    }
}

// 保存设置
async function saveSettings(shouldClose = false) {
    if (!validateTimerSettings()) {
        return;
    }

    const settings = {
        ...currentSettings,
        work_time: parseInt(workTimeInput.value),
        rest_time: parseInt(restTimeInput.value),
        long_work_threshold_mins: parseInt(longWorkThresholdInput.value),
        long_rest_mins: parseInt(longRestTimeInput.value),
        opacity: parseFloat(opacitySlider.value),
        language: languageSelect.value,
        auto_start: autoStartCheckbox.checked
    };

    try {
        await invoke('save_settings', { settings });
        currentSettings = settings;

        if (shouldClose) {
            await appWindow.close();
        }
    } catch (e) {
        console.error('Failed to save settings:', e);
        alert('保存失败: ' + e);
    }
}

// 透明度滑块变化事件
opacitySlider.addEventListener('input', (e) => {
    opacityValue.textContent = opacityToDisplay(e.target.value);
});

// 工作时间或阈值变化时实时校验
workTimeInput.addEventListener('input', () => validateTimerSettings());
longWorkThresholdInput.addEventListener('input', () => validateTimerSettings());

// 按钮点击事件
okBtn.addEventListener('click', () => saveSettings(true));
applyBtn.addEventListener('click', () => saveSettings(false));
cancelBtn.addEventListener('click', () => appWindow.close());

// 帮助函数：测量内容并请求主进程调整窗口大小（防止出现滚动条）
let resizeTimer = null;
async function requestResizeToContent() {
    await new Promise(r => setTimeout(r, 50));
    const body = document.body;
    const html = document.documentElement;
    const width = Math.ceil(Math.max(body.scrollWidth, html.scrollWidth));
    const height = Math.ceil(Math.max(body.scrollHeight, html.scrollHeight));
    try {
        await invoke('set_window_size', { width, height });
    } catch (e) {
        console.warn('set_window_size failed:', e);
    }
}

function scheduleResize() {
    if (resizeTimer) clearTimeout(resizeTimer);
    resizeTimer = setTimeout(requestResizeToContent, 80);
}

// 初始化
window.addEventListener('DOMContentLoaded', async () => {
    await loadSettings();

    listen('show-settings', async () => {
        await loadSettings();
    });

    scheduleResize();

    const ro = new ResizeObserver(() => scheduleResize());
    ro.observe(document.body);

    window.addEventListener('load', scheduleResize);
});
