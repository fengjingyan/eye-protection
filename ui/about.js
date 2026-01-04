const { invoke } = window.__TAURI__.tauri;
const { appWindow } = window.__TAURI__.window;
const { open } = window.__TAURI__.shell;

document.getElementById('closeBtn').addEventListener('click', () => {
    appWindow.hide();
});

document.getElementById('websiteLink').addEventListener('click', (e) => {
    e.preventDefault();
    open('https://github.com/fengjingyan');
});

document.getElementById('githubLink').addEventListener('click', (e) => {
    e.preventDefault();
    open('https://github.com');
});

async function init() {
    try {
        // In a real app, you might get this from tauri's getAppVersion
        // For now we can just use the one in the HTML or fetch it
    } catch (e) {
        console.error(e);
    }
}

init();
