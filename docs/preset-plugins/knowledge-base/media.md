# 多模态数据持久化与受控导出

[知识库目录](README.md) · [文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

## 数据路径

先上传核心Blob，再调用media-import(table_id,kind,blob_id)。核心检查声明Blob读取权，插件从HostService流式取回，校验长度/SHA256/MIME，将内容持久复制到本插件数据卷并返回media_id。

行的media值保存parts：text片段或`{type:image|audio|video,media_id:UUID}`，不能保存任意宿主路径/URL。media_id必须属于同表且模态一致，引用计数与行事务一并更新。

源Blob默认TTL24h不会删除已经复制的知识媒体。普通业务bytes/text也可存储，但不会自动被当作可向量化媒体。

## 类型与预算

首版接收image/jpeg、image/png、audio/wav、audio/flac、video/mp4，单次导入≤32MiB，实际格式必须与声明一致。存储可以保留超推理分辨率的合法媒体；向量化仍受所选embedding预设的解码/时长/token限额。

媒体读取、解码和导出均不能加载外部URL、压缩包路径、可执行代码或模型自定义脚本。所有CPU/临时盘/解码缓冲计入知识库或对应embedding实例。

## 导出与向量化

media-export对获准表的media_id创建临时核心Blob，返回Blob元数据；用户经核心downloadBlob下载。原存储不直接暴露HTTP文件目录。模型处理时知识库同样通过受控临时Blob再调用多模态Skill。

导出Blob归当前执行主体，过期后重新导出；不能把旧幂等结果的BlobID当永久链接。已删除源表不能撤回用户已经下载或仍在TTL内的合法导出副本。

删除行只减少引用；无引用媒体可由有界GC回收，table-drop显式清理所属媒体。插件升级/禁用/程序卸载不自动清理业务卷。
