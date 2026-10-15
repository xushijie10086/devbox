import test from "node:test";
import assert from "node:assert/strict";
import { isProdEnv, envChipView, envSwitchNotice } from "./envs.js";

const info = (active, names = ["dev", "test", "prod"], dflt = null) => ({
  active,
  default: dflt,
  profiles: names.map((name) => ({ name })),
});

test("没识别到环境的项目不显示标签", () => {
  assert.equal(envChipView(null), null);
  assert.equal(envChipView(undefined), null);
  assert.equal(envChipView({ active: null, profiles: [] }), null);
});

test("没选环境显示「默认」，并带上项目文件里本来的默认环境", () => {
  const v = envChipView(info(null, ["dev", "prod"], "dev"));
  assert.equal(v.text, "环境 默认");
  assert.equal(v.cls, "");
  assert.equal(v.title, "使用项目原本的环境（dev）（点击切换环境）");
  assert.equal(envChipView(info(null)).title, "使用项目原本的环境（点击切换环境）", "不知道默认环境就不写");
});

test("选了环境显示环境名，生产环境额外标红", () => {
  assert.deepEqual(envChipView(info("test")), { text: "环境 test", title: "当前环境：test（点击切换）", cls: "set" });
  assert.equal(envChipView(info("prod")).cls, "set env-prod");
  assert.equal(envChipView(info("Production", ["Production"])).cls, "set env-prod", "不区分大小写");
});

test("选中的环境已经从项目里消失：标出警告，并说明启动会被拒绝", () => {
  const v = envChipView(info("uat"));
  assert.equal(v.text, "环境 uat ⚠");
  assert.equal(v.cls, "set env-missing");
  assert.match(v.title, /启动会被拒绝/);
});

test("isProdEnv 只认生产类的名字", () => {
  for (const n of ["prod", "PROD", "production", "online", "release"]) assert.equal(isProdEnv(n), true, n);
  for (const n of ["dev", "test", "staging", "preprod", "products", "", null, undefined]) assert.equal(isProdEnv(n), false, String(n));
});

test("正在运行的项目切换环境后提示需要重启", () => {
  assert.deepEqual(envSwitchNotice("已切换到环境「test」", false), { text: "已切换到环境「test」", level: "success" });
  const r = envSwitchNotice("已切换到环境「test」", true);
  assert.equal(r.level, "warning");
  assert.match(r.text, /需点「重启」才会生效/);
});
