# 第三方许可补充材料

`supplements/` 保存 crate 发布包遗漏的许可证及拼音数据许可。各目录的 `provenance.json` 记录来源提交、URL 和 SHA256。

`scripts/collect-third-party-licenses.py` 离线校验并生成 `THIRD_PARTY_NOTICES.html`。依赖升级后检查许可兼容性；缺失正文可用 `--fetch-missing` 补齐并复核。

补充依据：

- `mac 0.1.1`：选择上游声明的 Apache-2.0 选项，保留发布版 README，附官方正文。
- `seahash 4.1.0`、`simd_helpers 0.1.0`：采用上游后续补入的 MIT 正文，分别记录发布提交和补充提交。
- `pinyin-data 0.13.0`：词表已与对应上游标签逐字节核对，许可来自同一提交。
