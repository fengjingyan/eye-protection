const invoke = window.__TAURI__.invoke;

// 获取DOM元素
const workTimeInput = document.getElementById('workTime');
const restTimeInput = document.getElementById('restTime');
const opacitySlider = document.getElementById('opacity');
const opacityValue = document.getElementById('opacityValue');
const autoStartCheckbox = document.getElementById('autoStart');
const okBtn = document.getElementById('okBtn');
const applyBtn = document.getElementById('applyBtn');
const cancelBtn = document.getElementById('cancelBtn');

const appWindow = window.__TAURI__.window.appWindow;
const listen = window.__TAURI__.event.listen;

let currentSettings = null;

// 加载设置
async function loadSettings() {
    try {
        // 从主进程获取设置
        currentSettings = await invoke('get_settings');
        
        // 填充设置到界面
        workTimeInput.value = currentSettings.work_time;
        restTimeInput.value = currentSettings.rest_time;
        opacitySlider.value = currentSettings.opacity;
        opacityValue.textContent = currentSettings.opacity;
        autoStartCheckbox.checked = currentSettings.auto_start;
    } catch (e) {
        console.error('Failed to load settings:', e);
    }
}

// 保存设置
async function saveSettings(shouldClose = false) {
    const settings = {
        ...currentSettings,
        work_time: parseInt(workTimeInput.value),
        rest_time: parseInt(restTimeInput.value),
        opacity: parseFloat(opacitySlider.value),
        auto_start: autoStartCheckbox.checked
    };
    
    try {
        // 发送设置到主进程
        await invoke('save_settings', { settings });
        currentSettings = settings;
        
        if (shouldClose) {
            await appWindow.hide();
        }
    } catch (e) {
        console.error('Failed to save settings:', e);
        alert('保存失败: ' + e);
    }
}

// 透明度滑块变化事件
opacitySlider.addEventListener('input', (e) => {
    opacityValue.textContent = e.target.value;
});

// 按钮点击事件
okBtn.addEventListener('click', () => saveSettings(true));
applyBtn.addEventListener('click', () => saveSettings(false));
cancelBtn.addEventListener('click', () => appWindow.hide());

// 帮助函数：测量内容并请求主进程调整窗口大小（防止出现滚动条）
let resizeTimer = null;
async function requestResizeToContent() {
    // 等待一次浏览器布局稳定
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

    // 初始调整
    scheduleResize();

    // 如果页面内容变化（比如用户更改表单项导致高度变化），监听并调整
    const ro = new ResizeObserver(() => scheduleResize());
    ro.observe(document.body);

    // 如果 window 大小或字体加载等导致变化，再次调整
    window.addEventListener('load', scheduleResize);
});
