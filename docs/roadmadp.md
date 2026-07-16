# kozue IR 拡張ロードマップ

## 目的

Mermaid / PlantUML の対応範囲を拡大しながら、次の要件を維持する。

- 同一入力から常に byte-identical な出力を生成する
- frontend → semantic IR → layout → renderer の境界を維持する
- source 固有の構文を共通 IR に無理に持ち込まない
- draw.io / Excalidraw / PPTX などへ意味情報を失わずに渡す
- serialized IR の互換性を明示的に管理する

すべての diagram を万能な graph に平坦化しない。`Diagram` は domain 別の
variant を維持し、ID、annotation、container、style など、本当に共有できる
概念だけを共通型として抽出する。

## 全体マイルストーン

### M1: Versioned IR document

状態: **実装済み、コミット済み**

- `IrDocument` と数値 wire version `IrSchemaVersion::V1` を追加
- `CURRENT_IR_SCHEMA_VERSION` を追加（M1 時点では V1、M2 で V2 へ更新）
- diagram name、title、description、accessibility metadata の受け皿を追加
- 決定的な `BTreeMap` ベースの namespaced `Extensions` を追加
- 未知 schema version を deserialize 時に拒否
- 新設 public 型を `#[non_exhaustive]` にして将来の Rust API 拡張に備える
- native DSL / Mermaid / PlantUML に `parse_document` を追加
- 既存 `parse(source) -> Diagram` と既存 `Diagram` wire 表現を維持
- native DSL 全5種と PlantUML の diagram name を保持
- Mermaid の name は V1 では `None`

V1 では `Extensions` の変更 API を公開しない。shape、relation kind、note、
fragment などの core semantics を extension に格納してはならない。

### M2: Stable element identity and annotations

状態: **実装済み、コミット済み**

- transparent newtype `ElementId` を導入
- 既存5図の named element、ordered map key、`from` / `to`、
  `Endpoint::State` を `ElementId` へ移行
- 対応する `SemanticLayout` の ID と endpoint も `ElementId` へ移行
- raw parser AST、diagnostic、Scene group 名、renderer 固有 ID は `String` のまま維持
- `IrDocument.annotations: Vec<Annotation>` を追加し、宣言順を保持
- diagram / single element / multiple elements を annotation target として型付け
- note、link、tooltip、stereotype、tag の共通 annotation payload を追加
- schema V2 を追加し、V1 document を空 annotations の V2 へ lossless upgrade
- M2 時点の serialize は V2（M3a1 で V3、M3a2a-I で V4、M3a2a-II で V5 へ更新）、未知 version、必須 field 欠落、
  未知 nested field を拒否
- bare `Diagram` の既存 JSON wire 表現と renderer 出力を維持

M2 では `PortId`、source provenance sidecar、style token、無名 relation / message /
transition 自体の ID、annotation の frontend 構文対応を延期した。annotation ID の重複、
dangling target、空の multi-element target の semantic validation も、実際に frontend が
annotation を生成するマイルストーンで追加する。

### M3: Existing diagram semantics

状態: **M3a2a-II（Graph circle / diamond shapes）実装済み**

既存5種を frontend ごとの最小 subset から、意味を保持できる IR へ拡張する。

1. Graph / Flowchart
   - **M3a1 実装済み**: Down / Right / Up / Left
     - native DSL `direction up|left`
     - Mermaid `BT` / `RL`
     - graph / class layout の主軸反転
     - DOT `rankdir=TB/LR/BT/RL`
     - schema V3 と V1 / V2 document migration
   - **M3a2a-I 実装済み**: legacy Default / Rectangle / RoundedRectangle
     - native DSL `shape rectangle|rounded`
     - Mermaid bare / `[label]` / `(label)` の shape 保持
     - layout と全 backend への shape 伝播
     - schema V4 と V1 / V2 / V3 document migration
   - **M3a2a-II 実装済み**: Circle / Diamond
     - native DSL `shape circle|diamond`
     - Mermaid `((label))` / `{label}` と明示宣言の last-wins 更新規則
     - shape 固有の sizing、Scene path、edge endpoint clipping
     - SVG / PNG / terminal / draw.io / DOT / Excalidraw / PPTX への shape 伝播
     - schema V5 と V1 / V2 / V3 / V4 document migration
   - **M3a2 次候補**:
     - その他の追加 shape、subgraph / container、port
     - source / target 両端 marker
     - dotted / dashed / thick などの line presentation
     - relation semantics と見た目を分離
2. Sequence
   - participant kind
   - note、activation、create / destroy
   - divider、delay、reference
   - `loop` / `alt` / `opt` / `par` / `critical` / `break` の再帰 fragment
   - open / filled / cross / circle / async / bidirectional arrow
3. State
   - composite state と region の階層構造
   - Initial / Final / Choice / Fork / Join / History の typed pseudostate
   - state description と internal behavior
   - transition の trigger / guard / effect
4. Class
   - member を visibility / name / type / parameter / modifier に構造化
   - namespace / package containment
   - association / generalization / realization / dependency を意味型で保持
5. ER
   - key と cardinality の型付け
   - direction、constraint、index metadata
   - layout 後も属性を整形済み文字列へ潰さない

### M4: Layout and exchange exporter contract

状態: **未着手**

- 現在の lossy な `SemanticLayout` を見直す
- layout output を `ElementId -> Geometry` mapping に寄せる
- exchange exporter に元の `Diagram` と geometry の両方を渡す
- shape、container、port、annotation、structured member を exporter まで保持
- Scene primitive に paint、stroke、ellipse、image / icon、semantic role を追加
- 未知 primitive や未対応 semantic item の silent skip を禁止する

### M5: Shared new diagram families

状態: **未着手**

共通性と利用価値の高い順に、domain 固有 variant として追加する。

1. Use case / Requirement
2. Component / Deployment / Architecture
3. Activity / Swimlane
4. Mindmap / Tree / WBS
5. Timeline / Gantt / Chronology
6. Network / structured data

### M6: Charts and specialized diagrams

状態: **未着手**

- Pie、XY、Radar、Quadrant、Sankey、Venn などは専用 semantic model を持つ
- Packet、Kanban、GitGraph、Timing、EBNF / Regex も必要に応じて独立 variant とする
- Salt、Ditaa、Math などは後順位とし、opaque passthrough を採用する場合も
  deterministic / hermetic な入力だけを許可する

## Source-specific extensions

共通 IR に正規化しない候補:

- Mermaid frontmatter、theme variables、renderer 指定、raw CSS、`classDef`
- PlantUML `skinparam`、`<style>`、Creole 原文、layout engine / pragma
- exact source round-trip 用の delimiter、quote、spelling
- preprocessor macro / include の定義情報

PlantUML の remote `!include`、`%load_json`、`%now`、Gantt の `today` などは
決定性要件と衝突する。preprocessor は IR の外に置き、外部入力はデフォルトで
禁止する。許可する場合は content hash と固定 evaluation context を要求する。

## Contract tests

各マイルストーンで以下を追加・維持する。

- frontend 間の等価入力が意味的に同一の IR になること
- IR schema fixture、round-trip、version migration / rejection
- 同一データの serialization が byte-identical であること
- 全 semantic element に対応 geometry が存在すること
- 対応 backend の golden test
- unsupported feature が silent downgrade / silent drop されないこと
- golden 更新時は `UPDATE_GOLDEN=1 cargo test` 後に必ず差分を確認すること

## M1 / M2 の検証状況

- `cargo fmt --check`: 成功
- `cargo check --workspace`: 成功
- `cargo test --workspace --no-run`: 成功
- `cargo test --workspace --exclude kozue-cli`: 成功
- `kozue-ir` の schema / migration / typed ID tests 9件: 成功
- CLI integration: 69 / 69 成功
- `cargo clippy --workspace --all-targets -- -D warnings`: 成功
- `git diff --check`: 成功

`drawio_class_er_goldens_match` で残っていた class / ER の draw.io golden は、
`value` 属性内の HTML を正しく XML escape した renderer 出力に合わせて更新済み。
独立レビューの指摘5件はすべて修正後に再レビュー済みで、blocking finding は
残っていない。

## M3a1 の検証状況

- `cargo test --workspace`: 成功
- CLI integration: 70 / 70 成功
- `kozue-ir` の schema / migration tests 12件: 成功
- graph / class の4方向、可変寸法、dummy route、bounds、決定性 tests: 成功
- `cargo clippy --workspace --all-targets -- -D warnings`: 成功
- `cargo fmt --check`: 成功
- `git diff --check`: 成功
- 独立レビューと2回の修正確認後、blocking findingなし

## M3a2a-I の検証状況

- schema V4 migration と旧 schema の明示 shape 拒否 tests: 成功
- native / Mermaid shape 等価性、formatter、unsupported shape tests: 成功
- layout kind 伝播、corner geometry、route 不変 tests: 成功
- SVG / PNG / terminal / draw.io / DOT / Excalidraw / PPTX mapping tests: 成功
- Mermaid `[label]` の Rectangle 化に伴う SVG golden のみ更新
- `cargo test --workspace` と workspace Clippy: 成功

## M3a2a-II の検証状況

- schema V5 migration と V1-V4 の node kind 互換性 matrix tests: 成功
- native / Mermaid の Circle / Diamond 構文、明示宣言更新規則、formatter tests: 成功
- sizing、固定 path 順序、解析的 endpoint clipping tests: 成功
- 全 backend mapping integration tests と `node_shapes` goldens: 成功

## 再開時の確認事項

1. M3a2 の追加 node shape と edge presentation の detailed design を作成する
2. subgraph / container / port の IR ownership と layout contract を確定する
3. annotation 構文対応前に `IrDocument` を layout / renderer へ渡す API を設計する
4. relation / message / transition 自体の安定 ID 生成規則を決める
