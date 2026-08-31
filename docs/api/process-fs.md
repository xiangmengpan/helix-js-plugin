# API:进程执行与异步文件系统

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

## 进程执行

### `helix.run(cmd)` — 同步（慎用）

同步执行 shell 命令并返回 stdout。**阻塞编辑器主线程**——只用于短命令。

```js
const cwd = helix.run("pwd").trim();
```

**优缺点**：优点：结果即用,简单。局限：**阻塞主线程**,重命令卡 UI;stdout 截断 64KB;Unix-only。

### `helix.run_async(cmd)` — 异步（返回 Promise）

worker 线程执行,不阻塞;Promise 恢复(`.then`/`.catch`/`await`)回到主线程事件循环。

```js
const out = await helix.run_async("git status --short");
helix.echo(out.trim());
```

- 成功 resolve stdout(字符串);非 0 退出码 reject `Error`(`e.message` 含 exit code)。

**优缺点**：优点：不阻塞;await 语法自然。局限：无流式输出(等命令结束);子进程无超时(挂死命令一直占 worker)。

### `helix.spawn({ cmd, pty?, onChunk, onExit })` — 流式进程

逐块输出(可 pty);返回进程 id,`helix.term_kill(id)` 可杀。

```js
const id = helix.spawn({
  cmd: "tail -f /var/log/syslog",
  pty: true,
  onChunk: (chunk) => { /* 增量显示 */ },
  onExit: (code) => { /* 清理 */ },
});
```

**优缺点**：优点：流式/pty;长跑命令友好。局限：需自己拼块;无超时;Unix-only。

## 异步文件系统

worker 线程执行,不阻塞编辑器。均返回 Promise:成功 resolve 值,失败 reject `Error`(`e.message` 可取消息)。

```js
helix.read_dir(path)                              // 同步!返回 [{name, is_dir, path}](不递归,按名排序)
helix.read_file_async(path)                       // Promise → 文件内容(UTF-8 lossy)
helix.write_file_async(path, content)             // Promise → undefined
helix.stat_async(path)                            // Promise → {is_dir, size, mtime}(mtime 为 Unix 秒)
helix.glob_async(pattern)                         // Promise → paths[](* / ** / ?,相对 CWD;** 跨目录)
helix.read_tree(path, { depth? })                 // Promise → [{name, is_dir, path}](递归;目录先行同级按名排序;depth 限深度,默认全递归)
```

```js
helix.read_dir("/tmp").forEach(e => {
  helix.echo((e.is_dir ? "[d] " : "    ") + e.name);
});
```

**优缺点**
- 优点：全部异步不阻塞;read_tree 递归 + 排序 + 深度限制(files 源基础);glob 支持 `**` 跨目录。
- 局限：`read_dir` 是同步的(不递归,列表短时可用);`read_file_async` UTF-8 lossy;glob 相对 CWD;read_tree 一次性返回(万级目录有延迟)。
