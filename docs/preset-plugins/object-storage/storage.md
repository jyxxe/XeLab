# 对象存储持久化与一致性

[对象存储目录](README.md) · [配置](configuration.md) · [接口](interfaces.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

插件使用独占single_writer卷`/data/objects`，SQLite WAL保存私有元数据、配额预留和幂等结果，内容保存在卷内不可变文件。逻辑结构见[storage.sql](storage.sql)，不在核心账户元数据表中复制密码、角色、凭证或原始委托。

## 导入与原子发布

1. 从可信RpcContext取得subject，校验key、mode、当前对象及expected_revision；锁定用户配额和对应逻辑key，建立有界持久预留。
2. 通过HostService.ReadBlob读取本次获准源Blob。请求中blob_id必须带受控资源标记，不能让插件自行读取任意UUID。
3. 在本数据卷创建随机私有临时文件；按连续offset、最多256KiB块复制，同时统计实际长度并计算完整SHA-256。
4. 超过声明长度、64MiB或当前预算立即停止；EOF时核对声明size_bytes/digest。零字节文件同样核对空内容摘要。
5. 对内容执行必要的fsync并原子rename到私有不可变对象位置；随后在事务中再次核对CAS，原子提交可见metadata、配额计数和稳定幂等结果。

不可见内容文件不等于成功对象。崩溃后可能存在临时文件或未被数据库引用的内容，恢复对账后有界清理；不得把扫描发现的文件自动发布为用户对象。发布后SQLite pointer必须指向完整持久内容，不允许先提交可见行再补写字节。

复制期间用预留防止并发请求超卖配额；使用短元数据事务建立/结束持久预留，不把SQLite写事务跨越整个流式复制。重启后按业务键和实际提交状态对账、恢复或释放，不能永久泄漏也不能按任意计时器释放仍在执行的预留。超时、取消和卷满在提交前清理未发布状态；提交后以已记录结果核实。

## create、replace与版本

每个(owner,key)保留永久head行，包括当前object_id或null、单调revision及live/deleted状态。该key从未创建时，create生成新object_id和revision=1；已删除key的create仍禁止expected_revision，但必须确认head处于deleted，再以新object_id和head.revision+1重建。

replace要求key处于live且expected_revision匹配，保持object_id、created_at并将head与可见对象revision加1。replace可修改内容和content_type，不能修改owner或key。head/tombstone不会作为普通内容GC删除，即使旧字节已完全回收，也不能让该key修订水位重新从1开始。

不同请求并发覆盖同revision时至多一个提交；新内容已准备但CAS失败的请求不得覆盖胜出者，只回收自己的不可见内容和预留。revision即使长度和摘要相同也按成功replace递增，不能用内容相同绕过CAS。

覆盖后旧内容立即退出新读取可见范围；仅已绑定旧内容的在途读取可以通过内部pin完成。旧内容不构成可查询的用户版本历史，释放最后一个引用后进入GC。删除重建时ID和revision均不能误匹配旧对象：例如可见rev7被删除后head为rev8，重建为新ID/rev9，旧expected_revision=7不能导出或删除rev9。

## 导出与临时Blob

export在同一受控读取中验证expected_revision并pin不可变内容，使用HostService.WriteBlob向核心流式创建临时Blob。成功返回BlobReference已有的ID、长度、摘要及本对象metadata中的必要字段，不编造Host接口未返回的创建/到期时间。

写入核心Blob前声明完整长度和摘要；核心只在字节校验通过后发布。源对象与导出Blob拥有独立生命周期：源对象删除不能撤销合法已导出、仍在TTL内的Blob或已下载副本；源Blob到期也不删除已持久导入内容。

export属于有副作用操作，必须记录业务键、输入指纹和固定结果。同键成功重放返回原blob_id；若其后Blob已过期，核心下载按过期状态失败，插件不能同键静默创建另一个Blob。调用者使用新键显式重导出。

核心Blob发布与插件SQLite提交不是一个跨存储事务。导出过程中崩溃或响应丢失时须利用核心调用/输出资源账本与插件结果记录核实；无法确认的调用保留unknown供查询或人工处理，不能仅凭“本地无成功行”盲目重建。未交付临时Blob按核心TTL回收。

## 幂等与删除

put、export和delete使用RpcContext稳定业务键，持久保存输入指纹、执行状态和安全结果；成功记录默认保留86400秒。每次重放先核实当前授权，再查原结果；匹配的成功重放先于当前对象CAS，因此原操作改变revision或已删除对象不会让安全重放失效。

同键不同指纹返回STATE_CONFLICT，不把新请求覆盖为旧结果。超过幂等记录保留期不保证去重，调用者应先核实Call和当前对象状态；更换Key、调用身份或业务键不意味着上次副作用不存在。

delete事务验证当前live对象的expected_revision，将head.revision加1并标记deleted/current_object_id=null，隐藏对象、扣减用户逻辑用量并保存`object_id/deleted_revision`结果；deleted_revision是这次删除消耗的新head水位，不是删除前可见值。物理内容在无在途pin后由有界GC删除，仍计入部署物理预算直至实际回收。

head/tombstone保留的元数据仍占部署/卷物理预算，不能因为逻辑内容已释放而忽略其容量；恢复与备份必须保留修订水位，不能仅凭“没有可见对象”删掉head。

## 分页、恢复与卷生命周期

list按key的UTF-8字节顺序进行keyset分页，每页独立读取当时的可见metadata。游标绑定subject、prefix、分页参数、数据域/部署代次及固定到期时间；不跨用户复用，不因读取续期。

分页不是持久全库快照：后续插入在已越过key之前的对象可能不出现在这次遍历，尚未读取的对象可能被删除或更新；同一次向前遍历不回退重复已越过的key。游标失效后重新从首页查询，不把失效游标解释成新首页。

禁用、重建、升级或卸载插件不自动删除业务卷。备份需停止写入、检查点WAL并生成一致内容清单，再复制数据库和全部被引用文件至宿主外；恢复核对数据库完整性、owner/key唯一性、revision、实际长度/摘要和配额后才Ready。

损坏内容不得以部分成功导出；以INTERNAL返回并保留诊断。迁移沿用核心备份、Apply、Verify与单写隔离流程，不在Initialize隐式变更持久格式，也不能只回退镜像却忽略磁盘格式。

物理布局也按可信owner UUID分区，例如users/<owner UUID>/content/<服务端内容UUID>；这些组件由服务端生成，逻辑key从不参与宿主路径拼接。os_contents记录owner，os_heads以(owner,content_id)复合外键约束同用户引用；共享服务进程仍属于受信边界，不等同每用户独立OS沙箱。
