// terminal 插件 node 级测试 —— 只钉"命令 → open_terminal 参数"这层接线。
//
// 运行:node --test plugins/terminal/tests/terminal.test.js   (node ≥ 24 不接受目录参数)
//
// 为什么值得钉:`at:"root"` 是"整宽/整高开终端"的唯一开关（核心按它决定切整窗还是切
// 当前窗口）。写丢了 ⇒ 命令看起来还能用，但终端只会在当前窗口里再裂一刀（半宽）。
'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');

/** 装一个最小桩，返回 {commands, opened} */
function load() {
  const commands = new Map();
  const opened = [];
  global.helix = {
    plugin() {},
    define_config() {},
    register_command(name, fn) {
      commands.set(name, fn);
    },
    // 配置一律给空对象 ⇒ 走默认尺寸
    get_config: () => ({}),
    run: () => '/bin/bash',
    open_terminal(opts) {
      opened.push(opts);
      return opened.length; // view_id
    },
    on() {},
    term_list: () => [],
    term_close() {},
    set_terminal_mode() {},
    echo() {},
  };
  // 每次重新加载（插件脚本顶层会注册命令）
  delete require.cache[require.resolve('../plugin.js')];
  require('../plugin.js');
  return { commands, opened };
}

test('hterm-root 在整窗底边开终端（at:"root" + side:"bottom"）', () => {
  const { commands, opened } = load();
  commands.get('hterm-root')();
  assert.equal(opened.length, 1);
  assert.equal(opened[0].side, 'bottom');
  assert.equal(opened[0].at, 'root', '必须带 at:"root"，否则只切当前窗口');
  assert.ok(opened[0].size > 0, 'size 要正数');
});

test('vterm-root 在整窗右侧开终端（at:"root" + side:"right"）', () => {
  const { commands, opened } = load();
  commands.get('vterm-root')();
  assert.equal(opened[0].side, 'right');
  assert.equal(opened[0].at, 'root');
});

test('旧的 hterm/vterm 保持"切当前窗口"（不带 at）', () => {
  const { commands, opened } = load();
  commands.get('hterm')();
  commands.get('vterm')();
  assert.equal(opened[0].at, undefined, '旧命令不能被顺手改成整窗');
  assert.equal(opened[1].at, undefined);
});
