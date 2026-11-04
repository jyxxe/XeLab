# M2 恢复后的历史用户重新激活

[实施与验收](README.md) · [恢复身份隔离](m2-platform-recovery.md) · [整体恢复](m2-platform-restores.md)

全平台恢复会清除历史密码和激活材料、撤销旧凭证及授权，并禁用恢复 Owner 以外的账户。
`xelabctl recover-account` 允许当前恢复 Owner 在宿主维护窗口为历史用户签发新的单次激活令牌。
用户通过原有 `activateAccount` 设置新密码，再通过 `login` 获取新会话。
这一步保留账户 UUID、归属和树结构；业务权限需要另行明确授权。

## 维护步骤

使用可信的当前二进制，按正常停机迁移流程升级到内部版本 **14**。已发布的版本 1–13
SQL 和校验和保持不变。新表保存激活申请、摘要和原始授权来源；再次恢复会撤销旧申请。
迁移也将早期恢复遗留的空 `selected` 继承修复为 `all_snapshot` 加空权限上限，权限仍为空，
同时恢复有效的公开 Account 表示。原生元数据快照要求精确版本和布局，旧版本快照不能直接导入版本 14。

1. 确认整体恢复已经完成，数据库恢复门禁为 `ready`。使用本轮恢复 Owner 的新密码登录，
   将返回的当前凭证保存为宿主 root 拥有的普通 0600 文件。凭证必须具有目标账户的
   `account.manage`，或严格后代账户的 `account.child.manage` 权限。
2. 通过 `getAccount` 核对目标账户 UUID、当前 `revision`、身份和父级。目标必须是恢复前
   已存在、未归档、未过期且密码已清除的 user，状态为 disabled 或 pending。
   目标不能是当前恢复 Owner，也不能保留 active 凭证；有父级时父级来源链必须有效。
3. 停止内核，等待宿主和数据库控制器租约释放。使用原有可信配置、DSN、HMAC 及受保护
   的密钥文件执行以下命令。保存明确的新 operation UUID 和核对过的 revision。

```bash
umask 077
xelabctl recover-account --config /etc/xelab/kernel.toml \
  --token-file /root/xelab-recovery-owner.token \
  --account-id <historical-user-uuid> \
  --confirm-account-id <same-historical-user-uuid> \
  --operation-id <new-operation-uuid> --revision <current-revision> \
  > /root/xelab-account-activation.json
```

4. 启动内核，将响应中的 `activation_token` 通过受保护渠道交给该用户。
   用户用 `activateAccount` 提交 `{"activation_token":"<token>","password":"<new-password>"}`，
   随后使用原账户 UUID 和新密码登录。激活必须在申请的期限内完成，期限最多一小时，
   并受原签发凭证链和目标账户有效期限制。
5. 按当前权限明确建立新的 Grant 或继承配置，重新签发所需凭证。旧密码、会话、API Key、
   Grant 和清空的继承上限不会恢复；其他独立根用户也不会因重新设置密码而重新获得 Owner。

命令同时要求实际宿主 root、独占宿主租约、空闲数据库控制器租约，以及经真实认证的当前
恢复 Owner 凭证。宿主权限不能代替账户授权，运行中的内核会阻止维护命令。

## 单次材料与撤销

明文激活令牌只在首次成功命令的 stdout 出现。数据库、Operation 和审计只保存摘要或非秘密
结果。相同 operation UUID 再次执行返回 `ONE_TIME_RESPONSE_UNAVAILABLE`，不会再返回秘密。
响应丢失时先检查账户；确需重新签发时重新读取 revision、使用明确的新 operation UUID，
新令牌会使旧令牌失效。修订不匹配或 operation UUID 已用于其他对象时拒绝执行。

激活时重新检查最初签发凭证、原权限路径、恢复代次、目标状态和父级来源链。
签发凭证到期或撤销后，后来新增的会话或授权不能挽救旧令牌；需要显式重新签发。
再次进行全平台恢复也会清除激活材料并撤销申请。账户的 active 凭证必须先通过常规流程撤销。

## 验证范围

真实 PostgreSQL 回归覆盖恢复后的子用户和独立根用户、旧密码/凭证拒绝、新密码登录、
单次响应、原签发者撤销后拒绝、明确重新签发、再次恢复撤销，以及不恢复旧业务权限。
版本 13 到 14 的回归使用已发布 SQL 建立旧布局，核对旧校验和、空继承上限和 Account 契约。
元数据快照往返和现有身份测试单独回归。

宿主 CLI 测试接入隔离 CI 的 XFS 测试步骤，执行实际 root/受保护文件检查、存活数据库 leader
拒绝、旧凭证拒绝、首次签发和重复响应拒绝。该测试的恢复门禁使用明确的元数据组件 fixture，
其最终提交的 CI 结果才是宿主命令执行证据。本页不代表真实 AWS 或独立宿主灾备验收完成。
