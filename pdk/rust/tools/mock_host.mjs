// Mock grain host（最终 ABI v0），guest 侧端到端验证用。
//
// 忠实模拟 host_runtime.dart 的语义：
// - 所有 import 是同步闭包；grain_http_request 等返回 host handle，操作异步完成
//   （这里用“第 2 次 event_wait 才完成”模拟宿主 await dio Future 的时延）
// - take_result 返回 0 = 未完成：宿主 await 自己的 Future 后再次轮询
// - 字符串一律 NUL 结尾；宿主读完指针调 guest.free_result(ptr)
// - 错误通道：latest 返回 0 / take_result 返回 0 时查 last_error()（NUL 结尾 JSON）
//
// Usage: node mock_host.mjs <plugin.wasm> <latest|search> [keyword]
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const wasmPath = process.argv[2];
const op = process.argv[3] ?? "latest";
const keyword = process.argv[4] ?? "鬼灭";

const wasm = readFileSync(wasmPath);
let guest, nextHandle = 1;
const pending = new Map(); // handle -> {kind, payload, polls}

const mod = await WebAssembly.instantiate(wasm, {
  grain: {
    grain_log(level, ptr, len) {
      const names = ["DEBUG", "INFO", "WARN", "ERROR"];
      console.log(`  [guest log/${names[level] ?? level}] ${readBytes(ptr, len)}`);
    },
    grain_http_request(reqPtr, reqLen) {
      const req = JSON.parse(readBytes(reqPtr, reqLen));
      requestedUrls.push(req.url);
      return startOp({ kind: "http", req });
    },
    grain_xpath_query(htmlPtr, htmlLen, xpathPtr, xpathLen) {
      return startOp({ kind: "xpath", xpath: readBytes(xpathPtr, xpathLen) });
    },
    grain_crypto_md5(ptr, len) {
      return startOp({ kind: "md5", data: readBytes(ptr, len) });
    },
    grain_crypto_base64_encode(ptr, len) {
      return startOp({ kind: "b64", data: readBytes(ptr, len) });
    },
    // 0=未完成；-1=buf 太小；-2=未知 handle；>0=已把响应 JSON 写入 buf 的字节数
    grain_event_wait(handle, bufPtr, bufCap) {
      const p = pending.get(handle);
      if (!p) return -2;
      p.polls = (p.polls ?? 0) + 1;
      if (p.polls < 2) return 0; // 模拟宿主 Future 尚未完成
      pending.delete(handle);
      const json = Buffer.from(buildResponse(p), "utf8");
      if (json.length > bufCap) return -1;
      writeBytes(bufPtr, json);
      return json.length;
    },
  },
});
guest = mod.instance.exports;

// wasm 内存增长会 detach 旧 buffer，每次访问都新建视图
function view() { return new DataView(guest.memory.buffer); }
function readBytes(ptr, len) { return Buffer.from(guest.memory.buffer, ptr, len).toString("utf8"); }
function readCstr(ptr) {
  let len = 0;
  const buf = guest.memory.buffer;
  while (new Uint8Array(buf)[ptr + len] !== 0) len++;
  return Buffer.from(buf, ptr, len).toString("utf8");
}
function writeBytes(ptr, bytes) { new Uint8Array(guest.memory.buffer).set(bytes, ptr); }

function startOp(payload) {
  const handle = nextHandle++;
  pending.set(handle, payload);
  return handle;
}

// ---- canned backends --------------------------------------------------------
const NOW_HTTP_DATE = "Wed, 18 Nov 2026 16:55:00 GMT"; // UTC 16:55 -> h = (16+9)%24 = 1
const requestedUrls = [];

function buildResponse(p) {
  if (p.kind === "http") return cannedHttp(p.req.url);
  if (p.kind === "xpath") return JSON.stringify({ results: [`mock-xpath(${p.xpath})`] });
  if (p.kind === "md5") return JSON.stringify({ value: createHash("md5").update(p.data, "utf8").digest("hex") });
  if (p.kind === "b64") return JSON.stringify({ value: Buffer.from(p.data, "utf8").toString("base64") });
  return JSON.stringify({ error: "unknown op" });
}

function envelope(body, status = 200) {
  return JSON.stringify({ status, headers: { Date: NOW_HTTP_DATE, "Content-Type": "application/json" }, body });
}

function cannedHttp(url) {
  if (process.env.MOCK_FAIL) {
    return JSON.stringify({ error: `mock network failure for ${url}` });
  }
  if (url.includes("360zy")) {
    return envelope(JSON.stringify({
      code: 1, msg: "数据列表", page: 1, pagecount: 1, limit: "20", total: 2,
      list: [
        { vod_id: 1001, vod_name: "鬼灭之刃 柱训练篇", vod_pic: "https://img.example.com/1.jpg", vod_remarks: "第8集" },
        { vod_id: 1002, vod_name: "迷宫饭", vod_pic: "https://img.example.com/2.jpg", vod_remarks: "完结" },
      ],
    }));
  }
  if (url.includes("/x/web-interface/nav")) {
    return envelope(JSON.stringify({
      code: 0, message: "0",
      data: { wbi_img: {
        img_url: "https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png",
        sub_url: "https://i0.hdslb.com/bfs/wbi/4932cddb0c149eddcf9021ffd11a5d96.png",
      } },
    }));
  }
  if (url.includes("/x/web-interface/popular")) {
    return envelope(JSON.stringify({
      code: 0, message: "0",
      data: { list: [
        { bvid: "BV1zz411c7oF", title: "热门视频甲", pic: "https://i0.hdslb.com/bfs/archive/c.jpg", owner: { name: "甲团队" } },
      ] },
    }));
  }
  if (url.includes("/wbi/search/type")) {
    return envelope(JSON.stringify({
      code: 0, message: "0",
      data: { result: [
        { bvid: "BV1xx411c7mD", title: '<em class="keyword">鬼灭</em>之刃 全集', pic: "//i0.hdslb.com/bfs/archive/a.jpg", author: "官方" },
        { bvid: "BV1yy411c7nE", title: 'Reaction <em class="keyword">鬼灭</em> 柱训练篇', pic: "https://i0.hdslb.com/bfs/archive/b.jpg", author: "up主小王" },
      ] },
    }));
  }
  return envelope("{}");
}

// ---- driver -----------------------------------------------------------------
console.log(`== mock host: ${wasmPath}`);

const metaPtr = guest.metadata_json();
console.log("metadata:", readCstr(metaPtr));
guest.free_result(metaPtr);

let handle;
if (op === "search") {
  const kw = Buffer.from(keyword, "utf8");
  const kwPtr = guest.alloc(kw.length + 1);
  writeBytes(kwPtr, kw);
  writeBytes(kwPtr + kw.length, Buffer.from([0])); // 显式 NUL：复用块不保证零填充
  handle = guest.search(kwPtr);
  guest.dealloc(kwPtr, kw.length + 1);
} else {
  handle = guest.latest(1);
}
console.log(`${op} -> handle 0x${(handle >>> 0).toString(16)}${handle === 0 ? " (immediate failure)" : ""}`);

let resultPtr = 0;
for (let i = 0; i < 200; i++) {
  resultPtr = guest.take_result(handle);
  if (resultPtr !== 0) break;
  // 未完成：宿主查 last_error 并 await 自己的 Future（这里用宏任务模拟时延）
  const errPtr = guest.last_error();
  if (errPtr !== 0) {
    console.log(`ERROR: ${readCstr(errPtr)}`);
    guest.free_result(errPtr);
    process.exit(1);
  }
  await new Promise((r) => setImmediate(r));
}
if (resultPtr === 0) {
  const errPtr = guest.last_error();
  if (errPtr !== 0) {
    console.log(`ERROR: ${readCstr(errPtr)}`);
    guest.free_result(errPtr);
  } else {
    console.log("TIMEOUT: take_result still pending after 200 polls");
  }
  process.exit(1);
}
console.log(`${op} result:`, readCstr(resultPtr));
guest.free_result(resultPtr);

// second latest to verify cached h parameter (360zy only)
if (op === "latest" && wasmPath.includes("360zy")) {
  const h2 = guest.latest(1);
  let p2 = 0;
  for (let i = 0; i < 200; i++) {
    p2 = guest.take_result(h2);
    if (p2 !== 0) break;
    await new Promise((r) => setImmediate(r));
  }
  console.log("latest#2 result:", p2 > 0 ? readCstr(p2) : `(no result, ptr=${p2})`);
  if (p2 > 0) guest.free_result(p2);
}

console.log("requested urls:");
for (const u of requestedUrls) console.log("  ", u);
