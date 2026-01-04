const invoke = window.__TAURI__.invoke;
const listen = window.__TAURI__.event.listen;

// 获取DOM元素
const countdownEl = document.getElementById('countdown');
const closeBtn = document.getElementById('closeBtn');

// 倒计时功能
async function startCountdown() {
    try {
        // 从主进程获取设置
        const settings = await invoke('get_settings');
        
        // 应用透明度设置
        if (settings.opacity !== undefined) {
            document.body.style.backgroundColor = `rgba(0, 128, 128, ${settings.opacity})`;
        }

        let remainingTime = settings.rest_time * 60; // 转换为秒
        
        // 更新倒计时显示
        function updateCountdown() {
            const minutes = Math.floor(remainingTime / 60);
            const seconds = remainingTime % 60;
            countdownEl.textContent = `${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}`;
            
            remainingTime--;
            
            if (remainingTime < 0) {
                clearInterval(timer);
                // Optional: Automatically close or just stay at 00:00
                // invoke('close_reminder'); 
            }
        }
        
        // 初始化显示
        updateCountdown();
        
        // 每秒更新一次
        if (window.timer) clearInterval(window.timer);
        window.timer = setInterval(updateCountdown, 1000);
    } catch (e) {
        console.error('Failed to start countdown:', e);
    }
}

// 关闭按钮点击事件
closeBtn.addEventListener('click', () => {
    // 通知主进程关闭提醒窗口
    invoke('close_reminder');
});

// 初始化
window.addEventListener('DOMContentLoaded', () => {
    startCountdown();
    listen('start-rest', () => {
        startCountdown();
    });
    listen('update-settings', (event) => {
        const settings = event.payload;
        if (settings && settings.opacity !== undefined) {
            document.body.style.backgroundColor = `rgba(0, 128, 128, ${settings.opacity})`;
        }
        // Optional: Update timer if rest_time changed? 
        // Usually we don't interrupt the current rest unless requested.
    });
});
