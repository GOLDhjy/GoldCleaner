# GoldCleaner

[![Build](https://github.com/GOLDhjy/GoldCleaner/actions/workflows/release.yml/badge.svg)](https://github.com/GOLDhjy/GoldCleaner/actions/workflows/release.yml)
[![Release](https://img.shields.io/github/v/release/GOLDhjy/GoldCleaner)](https://github.com/GOLDhjy/GoldCleaner/releases)
[![Downloads](https://img.shields.io/github/downloads/GOLDhjy/GoldCleaner/total)](https://github.com/GOLDhjy/GoldCleaner/releases)
[![Stars](https://img.shields.io/github/stars/GOLDhjy/GoldCleaner)](https://github.com/GOLDhjy/GoldCleaner/stargazers)
[![License](https://img.shields.io/github/license/GOLDhjy/GoldCleaner)](LICENSE)

一个基于 Tauri 2 + React + TypeScript 的 Windows C 盘清理工具。

## 功能概览

- 扫描磁盘可清理内容，展示分类与体积
- 详情列表支持勾选清理项与“打开位置”
- 支持按分类清理或仅清理详情中手动勾选的文件
  
  
![GoldCleaner](public/GoldCleaner.png)

## 使用方式

1. 点击“扫描磁盘”
2. 勾选要清理的分类，或进入详情勾选具体文件
3. 点击“开始清理”

提示：
- 清理为直接删除（不进回收站）
- 部分系统目录需要管理员权限

## 开发与运行

在仓库根目录执行：

```bash
npm install
npm run tauri dev
```

打包：

```bash
npm run tauri build
```

## 结构说明

- `src/`：React UI
- `src-tauri/`：Rust 后端与 Tauri 配置
