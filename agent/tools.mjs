import { execFile } from 'node:child_process';
import path from 'node:path';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { AgentError } from './server.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, '..');
const EXE = path.join(PROJECT_ROOT, 'dist', 'bin', 'agentcanvas.exe');

export const project = {
  id: 'agentcanvas',
  name: 'AgentCanvas',
  version: '1.0.0',
  summary: '本地图形设计与海报排版引擎：一句话海报生成、语义约束排版、图片批量算子处理、结构化检查与真文件导出。',
  port: 8899
};
export const projectId = 'agentcanvas';

function runCli(method, args = {}) {
  return new Promise((resolve, reject) => {
    if (!existsSync(EXE)) {
      return reject(new AgentError('engine_missing', `未找到可执行文件: ${EXE}`));
    }
    const cliArgs = [method];
    for (const [k, v] of Object.entries(args)) {
      if (v === undefined || v === null) continue;
      if (typeof v === 'object') {
        cliArgs.push(`${k}=${JSON.stringify(v)}`);
      } else {
        cliArgs.push(`${k}=${v}`);
      }
    }
    execFile(EXE, cliArgs, { cwd: PROJECT_ROOT, timeout: 120000 }, (err, stdout, stderr) => {
      if (err) {
        let msg = stderr ? stderr.trim() : (stdout ? stdout.trim() : err.message);
        try {
          const parsed = JSON.parse(stdout);
          if (parsed && (parsed.e || parsed.error || parsed.code)) {
            msg = parsed.e || parsed.error || parsed.code;
          }
        } catch { /* not json */ }
        return reject(new AgentError('exec_failed', msg));
      }
      try {
        const out = JSON.parse(stdout.trim());
        resolve(out);
      } catch (parseErr) {
        resolve({ raw: stdout.trim(), stderr: stderr.trim() });
      }
    });
  });
}

export const tools = [
  {
    name: 'agentcanvas.caps',
    description: '返回设计引擎的全部能力说明、布局预设(presets)、图片处理算子(image_ops)、字体数及环境限制。',
    input_schema: { type: 'object', properties: {}, additionalProperties: false },
    risk: 'read',
    handler: async () => runCli('caps')
  },
  {
    name: 'agentcanvas.doctor',
    description: '检查本机图形设计环境（CJK中文字体支持、渲染器、内存限制等）。',
    input_schema: { type: 'object', properties: {}, additionalProperties: false },
    risk: 'read',
    handler: async () => runCli('doctor')
  },
  {
    name: 'agentcanvas.poster',
    description: '核心一键成片：从标题、商品图与版式预设，一步完成工程创建、排版求解、检查修复并导出最终海报图片。',
    input_schema: {
      type: 'object',
      properties: {
        title: { type: 'string', description: '海报主标题文案' },
        image: { type: 'string', description: '主素材图片文件路径' },
        preset: { type: 'string', description: '版式预设：poster/square/story/banner/xhs/ecom-main/card 等' },
        f: { type: 'string', enum: ['png', 'jpg', 'svg', 'webp'], description: '输出图片格式，默认 png' },
        out: { type: 'string', description: '输出文件名或目录' }
      },
      required: ['title', 'out'],
      additionalProperties: false
    },
    risk: 'exec',
    handler: async (input) => runCli('poster', input)
  },
  {
    name: 'agentcanvas.project_create',
    description: '创建一个新的画布设计工程。',
    input_schema: {
      type: 'object',
      properties: {
        name: { type: 'string', description: '工程名称' },
        w: { type: 'number', description: '画板宽度（像素）' },
        h: { type: 'number', description: '画板高度（像素）' },
        preset: { type: 'string', description: '尺寸预设，例如 poster, square, banner' }
      },
      required: ['name'],
      additionalProperties: false
    },
    risk: 'write',
    handler: async (input) => runCli('project.create', input)
  },
  {
    name: 'agentcanvas.canvas_add',
    description: '向画板添加设计元素（文字/图片/几何形状），支持语义布局约束(intent: below/above/anchor/margin/safe_margin)。',
    input_schema: {
      type: 'object',
      properties: {
        p: { type: 'string', description: '工程 ID' },
        b: { type: 'string', description: '画板 ID（默认 b1）' },
        t: { type: 'string', enum: ['text', 'image', 'rect', 'ellipse', 'line', 'svg'], description: '元素类型' },
        text: { type: 'string', description: '文字内容（类型为 text 时）' },
        asset: { type: 'string', description: '素材路径或 asset ID（类型为 image 时）' },
        intent: { type: 'object', description: '语义布局约束，如 {anchor:"top",margin:80} 或 {below:"title_id"}' }
      },
      required: ['p', 't'],
      additionalProperties: false
    },
    risk: 'write',
    handler: async (input) => runCli('canvas.add', input)
  },
  {
    name: 'agentcanvas.layout_resolve',
    description: '求解画板内所有语义布局约束，计算最终精确几何坐标并返回变化与冲突。',
    input_schema: {
      type: 'object',
      properties: {
        p: { type: 'string', description: '工程 ID' },
        b: { type: 'string', description: '画板 ID（可选）' }
      },
      required: ['p'],
      additionalProperties: false
    },
    risk: 'write',
    handler: async (input) => runCli('layout.resolve', input)
  },
  {
    name: 'agentcanvas.inspect_validate',
    description: '对工程进行结构化排版与设计规则检查，返回文字越界、对比度不足、素材缺失等问题证据及自动修复 fix 命令。',
    input_schema: {
      type: 'object',
      properties: {
        p: { type: 'string', description: '工程 ID' },
        b: { type: 'string', description: '画板 ID（可选）' }
      },
      required: ['p'],
      additionalProperties: false
    },
    risk: 'read',
    handler: async (input) => runCli('inspect.validate', input)
  },
  {
    name: 'agentcanvas.render_preview',
    description: '为画板生成真实的光栅化 PNG 预览图并返回路径。',
    input_schema: {
      type: 'object',
      properties: {
        p: { type: 'string', description: '工程 ID' },
        b: { type: 'string', description: '画板 ID' }
      },
      required: ['p'],
      additionalProperties: false
    },
    risk: 'read',
    handler: async (input) => runCli('render.preview', input)
  },
  {
    name: 'agentcanvas.render_export',
    description: '导出画板为高清交付文件，返回真实文件路径、尺寸及 SHA256 校验和。',
    input_schema: {
      type: 'object',
      properties: {
        p: { type: 'string', description: '工程 ID' },
        b: { type: 'string', description: '画板 ID' },
        f: { type: 'string', enum: ['png', 'jpg', 'svg', 'webp'], description: '导出格式' },
        sc: { type: 'number', description: '缩放倍率（如 2 代表 2x 高清导出）' },
        q: { type: 'number', description: '质量 1-100' },
        o: { type: 'string', description: '输出文件路径' }
      },
      required: ['p', 'o'],
      additionalProperties: false
    },
    risk: 'exec',
    handler: async (input) => runCli('render.export', input)
  },
  {
    name: 'agentcanvas.image_op',
    description: '执行单项图片算子操作：裁剪、缩放、旋转、调整、色调、模糊、锐化、去除背景(remove_bg)等。',
    input_schema: {
      type: 'object',
      properties: {
        in: { type: 'string', description: '输入图片路径' },
        out: { type: 'string', description: '输出图片路径' },
        op: { type: 'string', description: '操作名：crop/resize/scale/pad/blur/sharpen/remove_bg 等' },
        args: { type: 'object', description: '操作具体参数' }
      },
      required: ['in', 'out', 'op'],
      additionalProperties: false
    },
    risk: 'exec',
    handler: async (input) => runCli('image.op', input)
  },
  {
    name: 'agentcanvas.image_chain',
    description: '以链式管道执行多个图片算子（例如 "crop 1:1 > scale 0.5 > sharpen 1.2 > watermark 品牌"）。',
    input_schema: {
      type: 'object',
      properties: {
        in: { type: 'string', description: '输入图片路径' },
        out: { type: 'string', description: '输出图片路径' },
        chain: { type: 'string', description: '管道表达式或算子数组' }
      },
      required: ['in', 'out', 'chain'],
      additionalProperties: false
    },
    risk: 'exec',
    handler: async (input) => runCli('image.chain', input)
  },
  {
    name: 'agentcanvas.batch',
    description: '批量执行设计操作，单次往返执行多个方法。',
    input_schema: {
      type: 'object',
      properties: {
        jobs: { type: 'array', description: '任务数组，每项形如 {m: "image.probe", path: "a.png"}' }
      },
      required: ['jobs'],
      additionalProperties: false
    },
    risk: 'exec',
    handler: async (input) => runCli('batch', input)
  }
];
