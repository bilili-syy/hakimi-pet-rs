# 分工

三路并行，各自一个 git worktree + 分支，互不干扰：

| 路 | 分支 | 工作目录 | 负责目录 |
|---|---|---|---|
| A 窗口与系统 | feat/platform | D:\download\luleme2\hakimi-pet-rs-platform | src/platform/ |
| B 服务与数据 | feat/services | D:\download\luleme2\hakimi-pet-rs-services | src/core/、src/services/ |
| C 渲染与行为（集成） | feat/render | D:\download\luleme2\hakimi-pet-rs | src/render/、src/behavior/、src/app.rs |

完成后各自 push 分支并 `gh pr create --base main`，由 C 合并。
