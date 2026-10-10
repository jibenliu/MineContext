<div align="center">

<picture>
  <img alt="MineContext" src="src/MineContext-Banner.svg" width="100%" height="auto">
</picture>

### MineContext: Create with Context, Clarity from Chaos

An open-source, proactive context-aware AI partner, dedicated to bringing clarity and efficiency to your work, study and creation.

[中文](README_zh.md) / English

<a href="https://bytedance.larkoffice.com/wiki/Hn6ewRnAwiSro7kkH6Sc1DMFnng">Community Best Practice</a> · <a href="https://github.com/volcengine/MineContext/issues">Report Issues</a> · <a href="https://bytedance.larkoffice.com/share/base/form/shrcnPAjJtlufuhBZGegll41NOh">Feedback</a>

[![][release-shield]][release-link]
[![][github-stars-shield]][github-stars-link]
[![][github-issues-shield]][github-issues-shield-link]
[![][github-contributors-shield]][github-contributors-link]
[![][license-shield]][license-shield-link]  
[![][last-commit-shield]][last-commit-shield-link]
[![][wechat-shield]][wechat-shield-link]

<a href="https://trendshift.io/repositories/15157" target="_blank"><img src="https://trendshift.io/api/badge/repositories/15157" alt="volcengine%2FMineContext | Trendshift" style="width: 250px; height: 55px;" width="250" height="55"/></a>

👋 Join our [WeChat / Lark / Red Note Group](https://bytedance.larkoffice.com/wiki/Hg6VwrxnTiXtWUkgHexcFTqrnpg)

🌍 Join our [Discord Group](https://discord.gg/tGj7RQ3nUR)

Build this repository's macOS artifact with <code>./scripts/package-macos-tauri.sh</code> (unsigned dmg) · upstream releases: <a href="https://github.com/volcengine/MineContext/releases">Releases</a>

</div>

Table of Contents

- [👋🏻 What is MineContext](#-what-is-minecontext)
- [🚀 Key Features](#-key-features)
- [🔏 Privacy Protection](#-privacy-protection)
  - [Local-First](#local-first)
  - [Local AI model](#local-ai-model)
- [🏁 Quick Start](#-quick-start)
  - [1. Installation](#1-installation)
  - [2. Enter Your API Key](#2-enter-your-api-key)
  - [3. Start Recording](#3-start-recording)
  - [4. Forget it](#4-forget-it)
  - [5. Backend Debugging](#5-backend-debugging)
- [🎃 Contribution Guide](#-contribution-guide)
  - [🎨 Frontend Architecture](#-frontend-architecture)
    - [Core Tech Stack](#core-tech-stack)
    - [Core Architecture](#core-architecture)
  - [💻 Frontend Usage](#-frontend-usage)
    - [Build Backend](#build-backend)
    - [Install Dependencies](#install-dependencies)
    - [Development and Debugging](#development-and-debugging)
    - [Application Packaging](#application-packaging)
  - [🏗️ Backend Architecture](#️-backend-architecture)
    - [Core Architecture Components](#core-architecture-components)
    - [Layer Responsibilities](#layer-responsibilities)
  - [🚀 Backend Usage](#-backend-usage)
    - [Installation](#installation)
    - [Configuration](#configuration)
    - [Running the Server](#running-the-server)
- [💎 The Philosophy Behind the Name](#-the-philosophy-behind-the-name)
- [🎯 Target User](#-target-user)
- [🆚 Comparison with Familiar Application](#-comparison-with-familiar-application)
  - [MineContext vs ChatGPT Pulse](#minecontext-vs-chatgpt-pulse)
  - [MineContext vs Dayflow](#minecontext-vs-dayflow)
- [👥 Community](#-community)
  - [Community and Support](#community-and-support)
- [Star History](#star-history)
- [📃 License](#-license)

<br>

> **🔗 Related Project**: Check out **[OpenViking](https://github.com/volcengine/OpenViking)** - An open-source Context Database designed for AI Agents. OpenViking unifies Memories, Resources, and Skills through a "file system paradigm", providing the infrastructure layer for sophisticated context management.

<br>

# 👋🏻 What is MineContext

MineContext is a proactive context-aware AI partner. By utilizing screenshots and content comprehension (with future support for multi-source multimodal information including documents, images, videos, code, and external application data), it can see and understand the user's digital world context. Based on an underlying contextual engineering framework, it actively delivers high-quality information such as insights, daily/weekly summaries, to-do lists, and activity records.

![feature.gif](src/feature.gif)

# 🚀 Key Features

MineContext focuses on four key features: effortless collection, intelligent resurfacing, proactive delivery, and a context engineering architecture.

1. 📥 Effortless Collection
   Capable of gathering and processing massive amounts of context. Designed storage management enables extensive collection without adding mental burden.
2. 🚀 Proactive Delivery
   Delivers key information and insights proactively in daily use. It extracts summarized content from your context—such as daily/weekly summaries, tips, and todos—and pushes them directly to your homepage.
3. 💡 Intelligent Resurfacing
   Surfaces relevant and useful context intelligently during creation. Ensures assisted creativity without overwhelming you with information.
4. 🎯 Context Engineering Architecture
   Supports the complete lifecycle of multimodal, multi-source data—from capture, processing, and storage to management, retrieval, and consumption—enabling the generation of six types of intelligent context.

# 🔏 Privacy Protection

## Local-First

MineContext places a high priority on user privacy. By default, all data is stored locally in the following path to ensure your privacy and security.

```
~/Library/Application Support/MineContext/Data
```

## Local AI model

In addition, we support custom model services based on the OpenAI API protocol. You can use fully local models in MineContext, ensuring that any data does not leave your local environment.

# 🏁 Quick Start

## 1. Installation

Click [Github Latest Release](https://github.com/volcengine/MineContext/releases) to Download

![Download APP](src/Download-App.gif)

> **Note**: The distributed macOS `.dmg` is **unsigned and not notarized**. If macOS says the app is “damaged” or cannot verify the developer, clear the quarantine attribute first:
>
> ```bash
> xattr -cr ~/Downloads/MineContext_*.dmg
> # if already copied to Applications:
> xattr -dr com.apple.quarantine /Applications/MineContext.app
> ```
>
> Or right-click the `.app` → Open. Once you have an Apple Developer ID and notarize builds, these steps can go away.

## 2. Enter Your API Key

After the application launches, please follow the prompts to enter your API key. (Note: On the first run, the application needs to install the backend environment, which may take about two minutes).

We currently support services from Doubao, OpenAI, and custom models. This includes any **local models** or **third-party model** services that are compatible with the OpenAI API format.

We recommend using [LMStudio](https://lmstudio.ai/) to run local models. It provides a simple interface and powerful features to help you quickly deploy and manage them.

**Considering both cost and performance, we recommend using the Doubao model.** The Doubao API Key can be generated in the [API Management Interface](https://console.volcengine.com/ark/region:ark+cn-beijing/apiKey).

After obtaining the Doubao API Key, you need to activate two models in the [Model Activation Management Interface](https://console.volcengine.com/ark/region:ark+cn-beijing/model): the Visual Language Model and the Embedding Model.

- Visual Language Model: Doubao-Seed-1.6-flash
  ![doubao-vlm-model](src/doubao-vlm-model.png)

- Embedding Model: Doubao-embedding-vision
  ![doubao-emb-model](src/doubao-emb-model.png)

The following is the filling process after obtaining the API Key:

![Enter API Key](src/Enter-API-Key.gif)

## 3. Start Recording

Enter [Screen Monitor] to enable the system permissions for screen sharing. After completing the setup, you need to restart the application for the changes to take effect.
![Enable-Permissions](src/Enable-Permissions.gif)

After restarting the application, please first set your screen sharing area in [Settings], then click [Start Recording] to begin taking screenshots.
![Screen-Settings](src/Screen-Settings.gif)

## 4. Forget it

After starting the recording, your context will gradually be collected. It will take some time to generate value. So, forget about it and focus on other tasks with peace of mind. MineContext will generate to-dos, prompts, summaries, and activities for you in the background. Of course, you can also engage in proactive Q&A through [Chat with AI].

## 5. Backend Debugging

MineContext supports backend debugging, which can be accessed at `http://localhost:1733`.

1.View Token Consumption and Usage
![后台调试1](src/backend-web-1.png)

2.Configure Interval for Automated Tasks
![后台调试2](src/backend-web-2.png)

3.Adjust System Prompt for Automated Tasks
![后台调试3](src/backend-web-3.png)

> **What this repository delivers.** A Rust daemon (`mc-daemon`) plus a **Tauri 2**
> desktop shell; the renderer is a plain Vite build. There is no Python backend and
> no Electron main process. Build, verify, troubleshoot and the release checklist live in
> [`docs/operations.md`](docs/operations.md); architecture and trade-offs in
> [`docs/architecture.md`](docs/architecture.md). The download buttons point at
> **upstream** releases; this working tree produces an **unsigned** dmg via
> `./scripts/package-macos-tauri.sh` (signing/notarization: docs/operations.md §5).

# 🎃 Contribution Guide

## 🎨 Frontend Architecture

The MineContext frontend is a cross-platform desktop application: a **Tauri 2** shell (Rust) hosting a React + TypeScript renderer that talks to the local `mc-daemon` over HTTP + SSE.

### Core Tech Stack

| Technology   | Description                                                                               |
| ------------ | ----------------------------------------------------------------------------------------- |
| Tauri 2      | Desktop shell: window, tray, single-instance lock, daemon lifecycle, packaging.           |
| React        | A component-based UI library for building dynamic user interfaces.                        |
| TypeScript   | Provides static type checking to enhance code maintainability.                            |
| Vite         | Renderer build tool (plain `vite`; output goes to `frontend/out/renderer`).               |
| Tailwind CSS | A utility-first CSS framework for rapid and consistent UI styling.                        |
| pnpm         | A fast and efficient package manager suitable for monorepo projects.                      |

### Core Architecture

The shell and the renderer are decoupled by exactly one contract: `runtime.json` (port + token) written by `mc-daemon`. The shell starts the daemon and injects `window.mcRuntime`; the renderer adapts IPC-style channel calls to HTTP/SSE, so business code never imports a shell API.

```
crates/         # Rust workspace: domain, storage, capture, pipeline, server, …
apps/           # mc-daemon (backend entry) and mc-cli (ops commands)
src-tauri/      # Tauri 2 shell (separate crate: tauri deps stay out of the root gate)
frontend/
├── src/renderer/   # React UI + adapters (IPC-style channels → HTTP/SSE)
├── packages/shared # shared config, channel enums, renderer logger
├── out/renderer/   # vite output = Tauri frontendDist (git-ignored)
└── scripts/        # frontend-side build helpers
scripts/        # gate (verify-all.sh), packaging, guards, tests
```

1.  **Tauri shell (`src-tauri/`) is responsible for:**

    - Starting/stopping `mc-daemon` and waiting for `runtime.json`
    - Window, tray (show window / quit), close-to-tray, single-instance lock, autostart
    - Injecting `window.mcRuntime` so the renderer can reach the daemon

2.  **Adapter layer (`src/renderer/src/adapters/`) is responsible for:**

    - Mapping IPC-style channels to daemon HTTP requests and SSE subscriptions
    - Installing the legacy globals (`dbAPI`, `screenMonitorAPI`, …) business code uses
    - Failing loudly for channels that have no implementation (no silent no-ops)

3.  **Renderer (`src/renderer/`) is responsible for:**

    - Implementing the user interface with React
    - Managing global state with Jotai and Redux
    - Utilizing an efficient styling system based on Tailwind CSS
    - Implementing dynamic loading and performance optimization mechanisms

4.  **Build and packaging:**

    - `frontend/vite.config.mts` — renderer build (`root`, `base: ./`, `outDir` aligned with Tauri `frontendDist`).
    - `./scripts/package-macos-tauri.sh` — release daemon → renderer → `cargo tauri build` → launch check (unsigned dmg).

## 💻 Development

### Requirements

| Need | Version | Notes |
| --- | --- | --- |
| Rust | 1.97+ | workspace in `crates/` and `apps/`; shell in `src-tauri/` |
| Node.js | 20+ | renderer build and tests |
| pnpm | 9+ | renderer dependencies |

macOS also needs Xcode Command Line Tools (screen capture and window metadata use system frameworks).

### Install dependencies

```bash
cargo fetch                  # Rust dependencies
cd frontend && pnpm install  # renderer dependencies
```

### Run it locally

```bash
# 1) build and run the daemon (it writes port + token into runtime.json)
cargo run -p mc-daemon

# 2) in another terminal: renderer only (vite dev server on port 5173)
cd frontend && pnpm dev

# 3) shell + renderer + daemon together (the real shape)
cargo tauri dev --manifest-path src-tauri/Cargo.toml
```

Enumerating capture targets is slow on first use (the OS is asked for displays and windows).

### Build the macOS installer

One command (recommended; includes the launch check):

```bash
./scripts/package-macos-tauri.sh
# artifact: src-tauri/target/release/bundle/dmg/MineContext_<version>_<arch>.dmg
```

Step by step (when you want control over each stage):

```bash
cargo build --release -p mc-daemon            # daemon binary used by the bundle
cd frontend && pnpm install && pnpm build     # renderer (output in frontend/out/renderer)
cd ../src-tauri
cargo tauri build --bundles dmg \
  --config '{"bundle":{"resources":{"../target/release/mc-daemon":"backend/mc-daemon"}}}'
# .app only (no dmg): --bundles app
```

The artifact is **unsigned and not notarized** (this repository has no Developer ID); the first
launch needs approval in System Settings → Privacy & Security. Signing/notarization is tracked in
`docs/operations.md` §5. App data lives in `~/Library/Application Support/com.minecontext.desktop`,
logs in `~/Library/Logs/com.minecontext.desktop/`.

### Build the Windows installer (not verified yet)

Non-macOS platforms have fallback branches (the capture layer reports "unsupported" instead of
crashing), but `tauri.conf.json` only declares the `dmg` target — so a Windows artifact has
**never been built or verified in this repository**. On a Windows machine, add the target and build:

```powershell
# run on Windows; needs Rust (MSVC), Node, pnpm and the WebView2 runtime
cargo build --release -p mc-daemon
cd frontend; pnpm install; pnpm build
cd ..\src-tauri
cargo tauri build --bundles nsis --config '{"bundle":{"resources":{"../target/release/mc-daemon.exe":"backend/mc-daemon.exe"}}}'
# artifact: src-tauri\target\release\bundle\nsis\*.exe (use --bundles msi for an MSI)
```

**Status, stated plainly**: screen capture, window metadata and clipboard reading are macOS
implementations (`crates/mc-capture/src/platform/`). On Windows the daemon starts, the UI opens and
retrieval/summaries work, but **no screen content is captured**. Making Windows a supported target
needs a capture implementation plus real-machine verification; neither exists yet
(registered in `docs/operations.md` §6).

## 🧪 Verification

```bash
./scripts/verify-affected.sh   # everyday changes: smallest relevant check set
./scripts/verify-all.sh        # full gate (end of a stage / before release)
./scripts/verify-external.sh   # items needing a real machine (SKIP ≠ PASS)
```

The full gate has eight steps: contract fixtures → guards and their self-test → `fmt` →
`clippy -D warnings` → full Rust tests (parallel) → macOS artifacts and real-process smoke →
frontend lint/types/three test layers/build → summary. What the gate cannot cover
(real-machine look and feel, signing, soak, golden dataset) is registered item by item in
[`docs/operations.md`](docs/operations.md) §6/§7 and is **never marked done because the code exists**.

## 🏗️ Backend Architecture

The backend is this repository's Rust workspace: `mc-daemon` is the only entry point and
assembles the other crates.

```
apps/mc-daemon    # daemon: HTTP control plane + SSE + capture loop + scheduler
apps/mc-cli       # ops commands: doctor / config / replay / import legacy / export-scenario
crates/mc-domain  # domain model: Event → Activity → Stage → Summary + projectors
crates/mc-storage # SQLite (migrations, single writer, event store, vectors, blob paths)
crates/mc-capture # capture sources: screen / window (+ clipboard, file if configured)
crates/mc-pipeline # change detection, privacy verdicts, queues and pools, AI extraction
crates/mc-providers # model access: OpenAI-compatible (chat / vision / embedding, real streaming)
crates/mc-search  # hybrid retrieval (keywords + vectors + filters)
crates/mc-summary # stage summaries and ad-hoc summaries (templates, fallback, evidence)
crates/mc-server  # control-plane routes, SSE event bus, chat engine
crates/mc-config  # layered config, validation, hot reload
```

Three invariants (breaking one is a bug):

1. **No stage without a summary**: every closed stage yields a non-empty summary (fallback template when no model).
2. **Offline by default**: with `privacy.ai_upload = false` none of the three network paths builds a provider.
3. **Secrets never in the database**: config stores references, values live in the system keychain.

## 🚀 Ops Commands

```bash
cargo run -p mc-cli -- doctor                 # environment and dependency self-check
cargo run -p mc-cli -- config show            # effective config with layer provenance
cargo run -p mc-cli -- replay --from-scratch  # rebuild derived tables from events
cargo run -p mc-cli -- import legacy          # import an old data directory (--dry-run supported)
```

# 💎 The Philosophy Behind the Name

The naming of MineContext also reflects the team's ingenuity. It signifies both "my context" and "mining context." It draws inspiration from the core philosophy of Minecraft—openness, creativity, and exploration.

If vast amounts of context are like scattered "blocks," then MineContext provides a "world" where you can freely build, combine, and create. Users can reimagine and create new content based on the collected massive context and generate high-quality information.

# 🎯 Target User

| Target User Category | Specific Roles/Identities          | Core Needs/Pain Points                                                                                   |
| -------------------- | ---------------------------------- | -------------------------------------------------------------------------------------------------------- |
| Knowledge Workers    | Researchers, Analysts              | Navigating vast amounts of information, improving information processing and analysis efficiency         |
| Content Creators     | Writers, Bloggers                  | Craving endless inspiration, optimizing content creation workflows                                       |
| Lifelong Learners    | Students, Researchers              | Building systematic knowledge systems, efficiently managing and connecting learning materials            |
| Project Managers     | Product Managers, Project Managers | Integrating multi-source information and data, ensuring project alignment and decision-making efficiency |

# 🆚 Comparison with Familiar Application

## MineContext vs ChatGPT Pulse

- 🖥️ Comprehensive Digital Context:
  MineContext captures your entire digital workflow by reading from screen screenshots, providing a rich, visual context of your daily activities and applications. ChatGPT Pulse, in contrast, is limited to the context of a single text-based conversation.
- 🔒 Local-First Data & Privacy:
  Your data is processed and stored entirely on your local device, ensuring complete privacy and security without relying on cloud servers. ChatGPT Pulse requires data to be sent to and stored on OpenAI's servers.
- 🚀 Proactive & Diverse Insights:
  MineContext delivers a wider variety of intelligent, auto-generated content—including daily summaries, actionable todos, and activity reports—not just simple tips. ChatGPT Pulse primarily offers reactive assistance within the chat interface.
- 🔧 Open Source & Customizable:
  As an open-source project, MineContext allows developers to freely inspect, modify, and build upon the codebase for complete customization. ChatGPT Pulse is a closed, proprietary product with no option for modification.
- 💰 Cost-Effective API Usage:
  MineContext avoids the need for a costly $200/month Pro subscription by allowing you to use your own API key, giving you full control over your spending. ChatGPT Pulse's advanced features are locked behind its expensive premium tier.

## MineContext vs Dayflow

- 💡 Richer, Proactive Insights:
  MineContext delivers a more diverse range of automated, intelligent content—including concise summaries, actionable todos, and contextual tips—going beyond basic activity tracking. DayFlow primarily focuses on logging user activity.
- 🧠 Context-Aware Q&A & Creation:
  MineContext enables you to ask questions and generate new content based on your captured context, unlocking wider application scenarios like content drafting and project planning. DayFlow is limited to passive activity recording and review.
- ✨ Superior Activity Generation & Experience:
  MineContext produces activity records with greater clarity and detail, featuring a more intuitive and interactive dashboard for a seamless user experience. DayFlow's activity logs are more basic with limited interactivity.

<br>

# 👥 Community

## Community and Support

- [GitHub Issues](https://github.com/volcengine/MineContext/issues): Errors and issues encountered while using MineContext.
- [Email Support](mailto:minecontext@bytedance.com): Feedback and questions about using MineContext.
- <a href="https://bytedance.larkoffice.com/wiki/Hg6VwrxnTiXtWUkgHexcFTqrnpg">WeChat Group</a>: Discuss SwanLab usage and share the latest AI technologies.

# Star History

[![Star History Chart](https://api.star-history.com/svg?repos=volcengine/MineContext&type=Timeline)](https://www.star-history.com/#volcengine/MineContext&Timeline)

# 📃 License

This repository is licensed under the Apache 2.0 License.

<!-- link -->

[release-shield]: https://img.shields.io/github/v/release/volcengine/MineContext?color=369eff&labelColor=black&logo=github&style=flat-square
[release-link]: https://github.com/volcengine/MineContext/releases
[license-shield]: https://img.shields.io/badge/license-apache%202.0-white?labelColor=black&style=flat-square
[license-shield-link]: https://github.com/volcengine/MineContext/blob/main/LICENSE
[last-commit-shield]: https://img.shields.io/github/last-commit/volcengine/MineContext?color=c4f042&labelColor=black&style=flat-square
[last-commit-shield-link]: https://github.com/volcengine/MineContext/commits/main
[wechat-shield]: https://img.shields.io/badge/WeChat-微信-4cb55e?labelColor=black&style=flat-square
[wechat-shield-link]: https://bytedance.larkoffice.com/wiki/Hg6VwrxnTiXtWUkgHexcFTqrnpg
[github-stars-shield]: https://img.shields.io/github/stars/volcengine/MineContext?labelColor&style=flat-square&color=ffcb47
[github-stars-link]: https://github.com/volcengine/MineContext
[github-issues-shield]: https://img.shields.io/github/issues/volcengine/MineContext?labelColor=black&style=flat-square&color=ff80eb
[github-issues-shield-link]: https://github.com/volcengine/MineContext/issues
[github-contributors-shield]: https://img.shields.io/github/contributors/volcengine/MineContext?color=c4f042&labelColor=black&style=flat-square
[github-contributors-link]: https://github.com/volcengine/MineContext/graphs/contributors
