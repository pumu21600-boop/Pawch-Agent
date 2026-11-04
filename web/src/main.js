import './lib/ui.css';
import { mount } from 'svelte';
import App from './App.svelte';

// Svelte 5 入口：mount() 取代 Svelte 4 的 new App()（034 迁移轮）。
// 组件仍按 legacy 语法编译（无 runes），行为不变。
const app = mount(App, { target: document.getElementById('app') });

export default app;
