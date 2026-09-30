# 采集器验证

## 离线单元测试

```bash
python -m unittest discover -s tests -p 'test_unit.py' -v
```

包含 26 项测试：schema 完整性、缺失与 false 的区别、零评分、TLS 部分字段缺失、
Rebrowser 未触发值、端点限制、输出脱敏、动作失败/超时和隔离执行异常处理，
以及 Chrome baseline 强制匿名 context、创建失败无 fallback、候选 existing 所有权。
这些是采集器测试，不是对候选浏览器反检测能力的判定。

## 本地 fixture 集成测试（可选）

需要本地 Chromium 可执行文件。测试会创建并最终删除自己拥有的临时 profile，
启动并停止自己拥有的 Chromium 子进程，不连接用户已有浏览器。

```bash
python tests/run_fixture_test.py --chromium /absolute/path/to/chromium
```

默认模式还启动仅监听 127.0.0.1 的临时 HTTP fixture，测试导航/响应与成功、超时、
坏 schema、非 JSON、429 等分支。所有数据有 LOCAL FIXTURE 标记，没有真实公网 TLS 测量。
同时验证新 baseline 不读取默认 profile 的 Cookie/localStorage、关闭自有 context、
保留原浏览器/标签页/存储。这些测试用的是 Chromium fixture，不代表 Obscura 兼容。
这个 fixture 运行器是采集器的开发测试辅助，不属于对实际浏览器的检测步骤。

只有需要在专用 root 容器里运行该测试时才可显式追加 `--no-sandbox`；
不要把这个参数抄到实际候选/基线的生产启动配置。

若运行环境策略禁止本地 HTTP 导航，不改变策略。可独立验证无导航的真实 DOM/CDP 契约：

```bash
python tests/run_fixture_test.py \
  --chromium /absolute/path/to/chromium \
  --dom-only \
  --report /absolute/path/outside/repo/fixture-validation.json
```

`--dom-only` 用浏览器的页面内容加载能力设置自有 fixture，验证公开对象读取、
DOM value、主/隔离执行环境、客户端断开后的浏览器和已有标签页保留。
**它不验证 HTTP 导航管线或线上 TLS 请求。**

## 上游包的历史记录

2026-09-30：21 项离线单元测试通过。
本地 Chromium 144.0.7559.96 + Playwright Python 1.57.0 的 5 项 DOM/CDP 契约检查通过，
这是原作者的构建记录；安装时不把原包生成的 `validation-result.json` 纳入 Git。

尝试完整本地 HTTP fixture 时，环境以 `net::ERR_BLOCKED_BY_ADMINISTRATOR` 阻止导航；
没有修改该限制。原包的 HTTP 集成测试和公网端到端运行当时尚未完成。
较新版 Playwright 的 `no_defaults` 分支按当前官方文档实现，但未在本构建环境运行。
没有连接用户的 Obscura 或其它实际候选浏览器，也没有声称它们通过检测。

## 本项目复核

2026-09-30：26 项离线单元测试通过；Chromium 148.0.7778.96 + 官方 Playwright
Python 1.60.0 的 9 项完整 HTTP fixture 检查通过。覆盖三个适配器导航采集、
Rebrowser 三种动作、超时/schema/非 JSON/429，以及匿名存储隔离和生命周期。
该轮运行了支持 `no_defaults` 的官方客户端分支。机器可读运行报告保留在仓库外。
公网站点与 Obscura 的完整差分验收不在这些结果中。

同日另以本项目 release Obscura、显式 `windows_chrome145` persona 和官方 Playwright
Python 1.60.0 运行三个适配器的本地观察轮：均 COMPLETE，原始 JSON、文本和截图
成功保存，自有匿名 context 正常关闭，无脚本或取证错误。该轮使用合成 HTTP fixture，
不是公网 TLS 指纹或 Bot 判断；Obscura 的 Rebrowser main/isolated 动作轮仍未取得资格。
