# Skel2D 개발 설계 문서 v2 — 메쉬 · 가중치 · 스킨 · 물리

작성: 2026-10-30 · 대상: 이 저장소를 이어서 구현할 에이전트
이전 문서: `docs/design/NEXT_PHASE_v1.md` (P0~P9 전체 로드맵과 P0~P1 진행 기록. 이번 범위 밖 항목의 상세 설계는 그쪽을 참고)

---

## 0. 목표와 범위

### 0.1 제품 목표
하나의 프로그램으로 다음 세 가지를 동시에 만족한다.
1. **AI가 쉽게 접근하고 제어할 수 있다.** 에디터에서 할 수 있는 일은 전부 op(`skel2d_core::Op`)로도 할 수 있고, CLI·HTTP(`serve`)·MCP에서 같은 op로 호출된다. 에디터 전용 코드가 유일한 수단이 되면 안 된다.
2. **2D 스켈레탈 애니메이션 에디터에 기대되는 기능을 구현한다.** 데이터 모델과 평가 결과는 널리 쓰이는 2D 스켈레탈 런타임의 4.2 세대 동작을 기준으로 맞춘다(내보내기 1:1 매핑 대비).
3. **작업 중 불편하기 쉬운 부분은 고친다.** 고친 지점은 코드 주석과 §6.3에 남긴다.

### 0.2 이번 범위
**메쉬, 가중치, 스킨, 물리**까지 완성한다. 각각 "기능 + AI op 동등성 + 개선된 에디터 UX"를 모두 갖춘 상태가 완료다.

범위 밖(보류, §5 백로그): IK 확장, 트랜스폼/패스 제약, 블렌드 모드·다크 틴트(wgpu 뷰포트), 애니메이션 보강(그래프 에디터·이벤트·드로우 오더 타임라인 등), 포인트/바운딩 박스/클리핑/패스 어태치먼트, 내보내기.
단, 이번 범위의 기능이 의존하는 부분(예: 물리 제약의 키 타임라인, 링크드 메쉬)은 이번에 구현한다.

---

## 1. 현재 구현 현황 (2026-10-30 코드 기준 점검)

v1 문서 기준 P0-1, P0-2, P0-3, **P1(셰어 + 상속 모드)까지 코드에 들어가 있다.** `cargo test` 대상 테스트 111개.

### 1.1 기반
| 항목 | 상태 | 위치 |
|---|---|---|
| 제약 순서(`order`, `set_constraint_order`, `ConstraintRef` 스케줄, 하위 트리 월드 재계산) | 완료 (IK만 등록) | `pose.rs::constraint_order`, `update_world_subtrees` |
| 대화상자 시스템(`DialogResult` + 7개 폼) | 완료 | `editor/dialogs.rs` |
| 트랙 순서 `Vec<LayerTrack>`, CLI/HTTP/MCP `layers` | 완료 | `model.rs::LayerTrack`, `pose.rs::preview_tracks` |
| 본 셰어 + 5개 상속 모드, 셰어 타임라인 | 완료 | `model.rs::Bone`, `Inherit`, `pose.rs::LocalTransform` |
| IK 1본/2본(mix, bend) | 기본만 | `ik.rs` (softness/stretch/compress 없음) |

### 1.2 메쉬
| 기능 | 상태 | 비고 |
|---|---|---|
| 모델 `Mesh { name, image, vertices(x,y,u,v,weights), triangles }` | 있음 | 정점은 셋업 포즈 스켈레톤 공간 |
| 리전 → 격자 메쉬 | op 있음 | `mesh_from_region { cols, rows }` |
| 이미지 외곽 트레이스 메쉬(알파 마스크, 밀도) | **에디터 전용** | `editor/import.rs::trace_mesh/retrace_mesh` → `skel2d-rig::mesh::outline_mesh` |
| 정점 추가(엣지 분할)/삭제/이동, Lawson flip, ear clip | **에디터 전용** | `editor/meshedit.rs::insert_vertex/remove_vertex/remove_vertices` |
| 내부 정점 자동 생성 | **에디터 전용** | `meshedit.rs::generate` |
| 정점 개별 수정 | op 있음 | `set_vertices`(위치·UV·가중치 패치, 추가/삭제 불가) |
| 메쉬 통째 교체 | op 있음 | `set_attachment` |
| 디폼 키 | op 있음 | `key_deform`, 오프셋은 스키닝 전 셋업 공간 |
| 헐(외곽) / 수동 엣지 | **없음** | 외곽은 매번 삼각형에서 추론 |
| UV 리셋, 트레이스 파라미터(알파 임계값·패딩) 노출 | **없음** | `OutlineParams`가 내부에만 있음 |
| 링크드 메쉬 | **없음** | |
| 메쉬 툴 모드(수정/생성/삭제) | 없음 | 현재 단일 모드 + 수정키 |

### 1.3 가중치
| 기능 | 상태 | 비고 |
|---|---|---|
| 거리 기반 자동 가중치 | op 있음 | `auto_weights { bones, max_influences, falloff, vertices, skin }` |
| 이미지 지오데식 자동 가중치 | **에디터 전용** | `editor/import.rs::auto_weights` → `skel2d-rig::weights::geodesic_weights` (CLI `rig`는 리깅 시 사용) |
| 위치 기반 가중치 전송 | op 있음 | `transfer_weights` |
| 좌우 미러 | op 있음 | `mirror_weights` |
| 브러시(Add/Remove/Replace, 반경·강도·목표·페더) | 에디터 | 결과는 `set_vertices`로 기록 (정상) |
| 스무스 / 프루닝(최대 본 수·임계값) / 본 교환 / 복사·붙여넣기 / 본 잠금 | **에디터 전용 알고리즘** | `meshedit.rs::smooth_weights/prune_weights/swap_weights/copy_weights/paste_weights` |
| 바인딩 본 목록 관리(본 추가·제거, "Bones" 목록) | 부분 | `bones_used`로 표시만 |
| 가중치 시각화(본별 색 / 선택 본 히트맵) | 있음 | 뷰포트 |

### 1.4 스킨
| 기능 | 상태 | 비고 |
|---|---|---|
| `Document.skins`, 플레이스홀더 기반 어태치먼트, skin-required 본, 스킨 IK | 완료 | `model.rs::Skin`, `resolve_attachment`, `bone_active` |
| 다중 스킨 동시 미리보기(나중 스킨 우선), 편집 자동 전환 | 완료 | `EditorApp::retarget_skins` |
| op: add/set/remove_skin, set/remove_skin_attachment, 메쉬 op의 `skin` 필드 | 완료 | |
| CLI/HTTP/MCP `--skin`/`skins` | 완료 | |
| 에디터 스킨 탭·인스펙터·슬롯 "스킨에 추가" | 완료 | `panels.rs::skins_view/skin_properties/slot_skins_section` |
| **디폼 키가 스킨을 구분하지 않음** | **버그** | `SlotTimelines.deform`가 어태치먼트 이름만으로 키잉 → 기본 스킨과 다른 스킨이 같은 플레이스홀더에 정점 수가 다른 메쉬를 두면 키가 충돌. 목표 구조는 `deform[skin][slot][attachment]` |
| 스킨 제약이 IK 전용(`Skin.ik`) | 부분 | 물리 제약 추가 시 일반화 필요 |
| 링크드 메쉬(스킨 간 기하·가중치 공유) | 없음 | |
| 어태치먼트 이름 변경 op | 없음 | v1 P0-2에서 보류 |

### 1.5 물리
전부 없음. 아이콘 `assets/icons/physics.png`만 생성돼 있고 `icons.rs`에 미연결.

### 1.6 점검 결론
- **AI 동등성 구멍이 가장 크다.** 메쉬 트레이스, 이미지 가중치, 정점 추가/삭제/생성, 스무스/프루닝/교환이 에디터에만 있다. AI는 메쉬를 통째로 계산해 `set_attachment`로 넣는 방법밖에 없다.
- 스킨 + 메쉬 조합에서 디폼 키 충돌 버그가 있다(물리보다 먼저 고친다).
- 물리는 포즈 계산을 상태 있는 시뮬레이션으로 바꾸는 구조 변경이라 마지막에 한다.

---

## 2. 핵심 구조 결정

### D1. 메쉬·가중치 알고리즘을 op 계층으로 승격
알고리즘을 두 종류로 나눈다.

| 종류 | 예 | 위치 | 실행 |
|---|---|---|---|
| **순수 기하**(문서만 필요) | 정점 추가/삭제, 재삼각화, 스무스, 프루닝, 교환, UV 리셋, 링크드 메쉬 해석 | 새 모듈 `skel2d-core/src/mesh.rs` | `skel2d_core::apply` 안에서 바로 실행 |
| **이미지 필요**(파일 읽기) | 외곽 트레이스, 지오데식 가중치 | `skel2d-rig` | 아래 "에셋 op" |

**에셋 op**: 코어는 파일 시스템을 모른다. 이미지가 필요한 op는 `skel2d-rig`에 `AssetOp` enum으로 두고, `skel2d_rig::expand(doc, base_dir, &[AssetOp]) -> Result<Vec<Op>>`가 이미지를 읽어 **일반 코어 op(`set_attachment` 등)로 펼친다**. 그 결과를 `skel2d_core::apply`로 원자 적용한다.
- 입력 JSON은 하나의 배열에 코어 op와 에셋 op를 섞어 쓸 수 있게 한다: `skel2d_rig::AnyOp = Core(Op) | Asset(AssetOp)` (`#[serde(untagged)]`가 아니라 `op` 태그 이름 공간을 공유, 이름 충돌은 테스트로 금지).
- CLI `apply`, HTTP, MCP `skel2d_apply_ops`, 에디터가 모두 `skel2d_rig::apply_any(doc, base, ops)` 한 함수를 쓴다. 에디터의 `import::trace_mesh/auto_weights`는 이 op를 만드는 얇은 래퍼로 바뀐다.
- 펼쳐진 결과(코어 op 목록)를 Undo 기록에 저장해 재현성을 보장한다(같은 이미지가 바뀌어도 Undo는 정확).
- `skel2d_schema`는 코어 + 에셋 op 스키마를 합쳐 낸다.

**규칙**: 에디터의 메쉬/가중치 버튼은 전부 op 하나를 만든다. 브러시처럼 연속 입력은 드래그 중 `app.preview`, 놓을 때 `set_vertices` 한 번(현행 유지).

### D2. 디폼 키를 스킨별로
- 모델: `SlotTimelines.deform: BTreeMap<String, Vec<Key<..>>>`(키 = 어태치먼트 이름)에 **스킨 차원 추가**. 새 필드 `Animation.deform: BTreeMap<String /*skin, 기본은 "default"*/, BTreeMap<String /*slot*/, BTreeMap<String /*attachment*/, Vec<Key<Vec<Vec2>>>>>>` (스킨 → 슬롯 → 어태치먼트 순의 중첩 모양).
- 마이그레이션(`lib.rs` 로드 단계): 기존 `slots.*.deform`을 `deform.default.*`로 옮긴다. 기존 키는 기본 스킨(슬롯 자체 어태치먼트)용이었으므로 의미 보존.
- 평가: 슬롯이 표시하는 어태치먼트를 해석할 때 **어느 스킨에서 왔는지**도 돌려준다(`Document::resolve_attachment_from -> (Option<&str> skin, &Attachment)`), 디폼은 그 스킨의 키를 쓴다.
- op: `key_deform`, `remove_key`, `clear_timeline`에 `skin: Option<String>` 추가(없으면 기본). 스킨/어태치먼트 이름 변경·삭제 시 참조 정리.
- 이름 `default`는 스킨 이름으로 예약(`validate`에서 금지).

### D3. 스킨 제약 일반화
- `Skin.ik: Vec<String>` → `Skin.constraints: Vec<String>`(모든 종류 제약 이름). 로드 시 `ik` → `constraints` 마이그레이션, 직렬화는 `constraints`만.
- 제약 이름은 종류와 관계없이 문서 전체에서 유일(`validate`). 이미 IK와 물리가 같은 `ConstraintRef` 스케줄에 오르므로 이름 공간도 하나로.
- 스킨 소유 제약: 해당 스킨이 비활성이면 스케줄에서 제외(현 IK 동작을 일반화).

### D4. 물리: 순수 포즈 + 상태 있는 스텝 (`Physics` 모드 개념)
```rust
pub enum PhysicsMode { None, Reset, Update, Pose }   // 물리 모드 enum
pub struct PhysicsState { bodies: Vec<BodyState>, time: f32 }   // 제약 순서대로
impl Pose {
    pub fn compute_skinned(doc, tracks, skins) -> Pose;                    // 기존, 물리 None (호출자 불변)
    pub fn compute_physics(doc, tracks, skins, state: &mut PhysicsState,
                           mode: PhysicsMode, delta: f32) -> Pose;         // 실시간
    pub fn simulate(doc, tracks_at: impl Fn(f32) -> Vec<Track>, skins,
                    t: f32, fps: f32) -> (Pose, PhysicsState);             // 결정적 재현
}
```
- `compute_skinned`는 내부적으로 `compute_physics(.., PhysicsMode::None, 0)`을 부른다(코드 경로 하나).
- 시간·스텝은 `f32` 누적 오차를 피하려고 **정수 스텝 카운트**로 진행한다(`steps = floor((time) / step)`), 결정성 테스트 대상.
- 스켈레톤 전체 이동(`physicsTranslate/physicsRotate`)을 `PhysicsState`에 받는 API를 둔다: 에디터에서 루트를 끌면 머리카락이 흔들리고, AI는 `simulate`에 "스켈레톤 이동 경로"를 줘서 같은 결과를 재현한다.

---

## 3. 단계별 설계

각 단계는 **모델 → 평가 → op → 검증 → CLI/HTTP/MCP → 에디터 UI → 테스트 → `docs/AI_GUIDE.md`** 순서로 진행하고, 독립적으로 빌드·테스트 가능한 상태로 끝낸다.

| 단계 | 내용 | 의존 |
|---|---|---|
| **M0** | 디폼 키 스킨별(D2) + 어태치먼트 이름 변경 op | 없음. 버그 수정이라 먼저 |
| **M1** | 메쉬 op 승격(D1 순수 기하) + 헐/엣지 모델 | M0 |
| **M2** | 에셋 op(트레이스·지오데식) + 트레이스 파라미터 | M1 |
| **M3** | 가중치 op 승격 + 바인딩 본 관리 + 가중치 UX | M1 |
| **M4** | 스킨 보강: 링크드 메쉬, 제약 일반화(D3), 스킨 UX | M0, M1 |
| **M5** | 물리 제약(D4) | M4(D3) |
| **M6** | 마감: AI 가이드 예제, 예제 파일, 통합 QA | 전부 |

---

### M0. 디폼 키 스킨별 + 어태치먼트 이름 변경

**모델/마이그레이션**: D2 그대로. 마이그레이션 테스트: v1 형식 파일(슬롯 디폼 키 포함)을 열면 `deform.default`로 옮겨지고 포즈가 동일.

**op**
- `key_deform { animation, slot, attachment, skin?, time, offsets, curve }`
- `rename_attachment { slot, from, to, skin? }`: 해당 어태치먼트의 이름, 슬롯 셋업 `attachment`, 모든 애니메이션의 어태치먼트 키, 디폼 키 경로를 갱신. `skin` 없으면 기본 스킨의 어태치먼트. 플레이스홀더 이름을 바꾸려면 모든 스킨에서 같이 바꾸는 `rename_placeholder { slot, from, to }`를 별도로 둔다(플레이스홀더와 어태치먼트 이름이 분리되면 혼란스러우므로 "이 스킨의 것만 / 모든 스킨" 두 op로 명확히).

**에디터**: 어태치먼트 행 F2 → 이름 변경 대화상자(기존 `dialogs.rs` 폼 재사용)에 "모든 스킨의 같은 플레이스홀더도 변경" 체크.

**테스트**: 기본 스킨 메쉬(정점 4개)와 스킨 `armor` 메쉬(정점 9개)가 같은 플레이스홀더일 때, 각자 디폼 키가 독립적으로 동작.

---

### M1. 메쉬 op 승격 + 헐/엣지

**모델** (`Mesh`)
- `hull: u32` — 앞쪽 `hull`개 정점이 외곽(시계 방향 순서). 일반적인 규약. 기본 0 = "미지정, 삼각형에서 추론"(기존 파일 호환). 로드 시 정점을 재배열하지 않는다(디폼 키 인덱스가 깨지므로). 새로 만들거나 재삼각화하는 op만 정점을 헐 순서로 배치하고 `hull`을 채운다.
- `edges: Vec<[u32; 2]>` — 사용자 제약 엣지(내부 선). 삼각화에서 flip 금지.
- `width/height`는 저장하지 않는다(이미지 크기에서 얻음).

**코어 모듈 `skel2d-core/src/mesh.rs`** (에디터 `meshedit.rs`의 기하 함수 이동)
- `triangulate(mesh)`: 헐 + 제약 엣지로 제약 들로네. `spade`를 코어 의존성으로 옮긴다(이미 워크스페이스 의존, 순수 Rust). 에디터의 Lawson flip/ear clip은 이 함수로 대체.
- `insert_vertex`, `remove_vertices`, `generate_interior(count | spacing)`, `reset_uvs`, `reset_to_uvs`.
- 새 정점의 UV는 기존처럼 인접 삼각형에서 보간, 가중치도 보간(`meshedit::blend` 이동).

**op**
| op | 내용 |
|---|---|
| `add_mesh_vertices { slot, attachment, skin?, points: [[x,y]], on_edge_tolerance? }` | 내부/엣지 위 추가, 재삼각화 |
| `remove_mesh_vertices { slot, attachment, skin?, indices }` | 삭제, 재삼각화, 디폼 키 인덱스 재매핑 |
| `set_mesh_edges { slot, attachment, skin?, edges }` | 제약 엣지 교체 |
| `generate_mesh_vertices { slot, attachment, skin?, count? , spacing? }` | 내부 정점 자동 생성 |
| `reset_mesh { slot, attachment, skin?, mode: "vertices_to_uvs" \| "uvs_to_vertices" }` | 가중치 유지 |

- **모든 정점 수를 바꾸는 op는 디폼 키를 재매핑한다**(삭제된 정점 키 제거, 추가된 정점 오프셋은 이웃 보간). 현재 `fit_deform_keys`는 길이만 맞춘다 — 인덱스 재매핑 테이블을 받도록 확장.

**에디터**
- 메쉬 툴 창(`mesh_tool_window`) 상단에 모드 세그먼트 **수정 / 생성 / 삭제 / 엣지**. 우클릭으로 순환. 현 수정키 동작은 유지.
- 엣지 모드: 정점→정점 드래그 = 엣지 추가, 엣지 클릭 + Del = 제거. 헐은 굵은 선, 제약 엣지는 주황 선, 일반 삼각형 엣지는 흐린 선.
- "UV로 리셋" / "정점에 UV 맞춤" 버튼 + 팔레트 명령.
- 정점 추가·삭제·생성·재생성이 가중치와 디폼 키를 보존(리셋하더라도 가중치를 지우지 않는다).

**테스트**: 재삼각화가 제약 엣지를 유지, 헐 밖 삼각형 없음, 삭제 후 디폼 키 길이·값 재매핑, 에디터 이전 결과와 동일한 기하(회귀).

---

### M2. 에셋 op (트레이스 · 지오데식 가중치)

**`skel2d-rig::AssetOp`**
| op | 내용 |
|---|---|
| `trace_mesh { slot, attachment, skin?, name?, alpha_threshold=0.05, padding=2, detail=0.5, concavity?, interior: "none"\|"grid"\|"auto", spacing?, keep_weights=true }` | 이미지 알파 외곽 → 헐, 내부 정점, 삼각화. 리전이면 메쉬로 교체, 메쉬면 재트레이스. `keep_weights`면 기존 가중치를 위치 기반 전송(`transfer_weights` 로직 재사용) |
| `image_weights { slot, attachment, skin?, bones?, max_influences=4, falloff=3, vertices? }` | 이미지 실루엣 내부 지오데식 거리 기반 가중치(팔 사이 빈 공간을 건너지 않음) |

- `OutlineParams`를 공개 파라미터로 정리하고 `Density` 프리셋(낮음/중간/높음)은 `detail`/`spacing` 값 묶음으로 매핑.
- 기존 `editor/import.rs::trace_mesh/retrace_mesh/auto_weights/weights_from_image`는 AssetOp 생성으로 대체하고 로직은 rig로 옮긴다.

**CLI/MCP**: 별도 도구를 늘리지 않는다. `apply`/`skel2d_apply_ops`가 AnyOp를 받는다. 결과 응답에 펼쳐진 요약(정점 수, 삼각형 수, 가중치 본 목록)을 포함해 AI가 다음 단계를 판단하게 한다.

**에디터**: P0-2의 메쉬 생성 대화상자에 알파 임계값·패딩·디테일·내부 정점 방식·"가중치 유지"를 연결하고, **대화상자 열린 동안 뷰포트에 결과 미리보기**(op를 `app.preview`로 적용). 수치를 바꿀 때마다 결과를 다시 눌러 봐야 하는 별도 트레이스 버튼은 두지 않는다.

**테스트**: 테스트 이미지(`examples/arm`)로 트레이스 결과의 헐이 알파 경계 안쪽 `padding` 이내, 결정적(같은 입력 → 같은 메쉬), CLI와 에디터 결과 바이트 동일.

---

### M3. 가중치 op 승격 + UX

**op** (코어, `mesh.rs`에 알고리즘)
| op | 내용 |
|---|---|
| `smooth_weights { slot, attachment, skin?, vertices?, factor=0.5, iterations=1, locked_bones? }` | 이웃 평균 |
| `prune_weights { slot, attachment, skin?, vertices?, max_bones=4, threshold=0.01, locked_bones? }` | 응답에 제거 개수 |
| `swap_weights { slot, attachment, skin?, vertices?, a, b }` | |
| `set_bone_weight { slot, attachment, skin?, vertices, bone, value, mode: "set"\|"add"\|"scale", locked_bones? }` | 수치 입력·AI용 (브러시의 이산 버전) |
| `bind_bones { slot, attachment, skin?, add?: [bone], remove?: [bone], redistribute: "auto"\|"nearest"\|"parent" }` | "Bones" 목록. 제거 시 그 본의 가중치를 재분배 |
| `normalize_weights { slot, attachment, skin? }` | |

- `locked_bones`는 op 인자로만 존재(문서에 저장하지 않음). 에디터의 잠금 상태는 세션 설정이고 op 생성 시 전달.
- 가중치 합 규칙: 평가 시 정규화(현행), op 결과는 항상 정규화된 값 저장, 반올림 4자리(현행 `round4`).

**에디터 가중치 창** (`weights_panel` 재구성, 툴 창 패턴)
```
┌ 가중치 · arm_mesh ────────────────────────┐
│ 바인딩 본            [+ 본 추가] [선택 본 제거]│
│  ● upper_arm  ■■■■■□  62%   🔒             │
│  ● forearm    ■■□□□□  31%   🔓             │
│  ● hand       □□□□□□   7%   🔓             │
│ 브러시  (•)추가 ( )제거 ( )대체   반경 40 강도 0.5│
│ 선택 정점 3개: [값 0.50] [설정] [스무스] [프루닝…] │
│ [자동(거리)] [자동(이미지)] [미러…] [전송…]       │
└──────────────────────────────────────────┘
```
- 본 행의 막대 = 선택 정점(없으면 전체)의 평균 영향도. 행 클릭 = 브러시 대상 본, 뷰포트에서 해당 본 히트맵.
- 프루닝·미러·전송은 P0-2 대화상자(옵션 + 영향 미리보기 "정점 12개에서 본 9개 영향 제거").
- **포즈 테스트 토글**: 가중치 툴에서 셋업을 벗어나 본을 임시 회전해 변형을 보며 칠하기(놓으면 셋업 복귀, 문서 불변).
- 가중치 툴에서 본 잠금이 브러시·스무스·프루닝·자동 가중치 모두에 일관 적용. 자동 가중치는 선택 정점에만 적용 가능.

**테스트**: 각 op 수치 기대값, 잠금 본 값 불변, 정규화 합 1, `bind_bones` 제거 후 그 본 가중치 0.

---

### M4. 스킨 보강

**링크드 메쉬**
- 모델: `Attachment::LinkedMesh { name, image: Option<String>, parent: String /*같은 슬롯의 메쉬 어태치먼트 이름*/, parent_skin: Option<String>, timelines: bool /*부모 디폼 키 공유, 기본 true*/ }`.
- 평가: 해석 시 부모 메쉬의 정점·삼각형·가중치·헐을 쓰고 이미지만 교체. `timelines=true`면 디폼 키를 부모(어태치먼트·스킨 경로)에서 읽는다(`timelines` 플래그).
- 검증: 부모가 존재하는 메쉬여야 함, 링크드 → 링크드 체인 금지, 순환 금지.
- op: `add_linked_mesh { slot, name, parent, parent_skin?, skin?, image, timelines }`. 부모 삭제 시 링크드 메쉬를 독립 메쉬로 굳힐지 함께 삭제할지 op 인자(`on_parent_removed`)로 받고, 기본은 오류(조용한 파괴 금지).
- 에디터: 메쉬 인스펙터에 "링크드 메쉬 만들기"(대상 스킨 선택 대화상자), 링크드 메쉬 인스펙터는 부모로 이동 버튼 + 기하 편집 툴 비활성 + 안내 문구.

**제약 일반화**: D3. 스킨 인스펙터 "스킨 제약" 그룹이 모든 종류 제약을 아이콘과 함께 나열.

**스킨 UX 개선**
- 스킨 인스펙터 어태치먼트 목록을 "슬롯 ▸ 플레이스홀더 ▸ 어태치먼트" 트리(`tree_row`)로. 비어 있는 플레이스홀더(다른 스킨엔 있는데 이 스킨엔 없는 것)를 흐리게 표시 — 스킨 간 누락을 한눈에.
- "스킨 비교" 팔레트 명령: 두 스킨의 플레이스홀더 차이를 상태바/대화상자로.
- `skel2d_pose`/`info` 응답에 활성 스킨별 해석 결과(슬롯 → 실제 어태치먼트, 출처 스킨) 포함 → AI가 스킨 결과를 수치로 검증.

**테스트**: 링크드 메쉬가 부모 정점 편집을 반영, `timelines` 켬/끔에 따른 디폼, 부모 삭제 정책, `ik`→`constraints` 마이그레이션.

---

### M5. 물리 제약

#### 모델 (`PhysicsConstraintData`)
```
Document.physics: Vec<PhysicsConstraint>
PhysicsConstraint {
  name, order: i32, bone: String,
  x, y, rotate, scale_x, shear_x: f32,   // 속성별 영향도 0..1 (0 = 그 속성 시뮬레이션 안 함). 기본값은 4.2 형식 기본값과 동일하게(착수 시 런타임 소스로 확인)
  limit: 5000, step: 1/60,
  inertia: 1, strength: 100, damping: 1, mass_inverse: 1, wind: 0, gravity: 0, mix: 1,
  inertia_global, strength_global, damping_global, mass_global, wind_global, gravity_global, mix_global: bool,
}
Document.physics_globals: PhysicsGlobals { wind, gravity, ... }   // "global" 플래그가 참조
Animation.physics: BTreeMap<String /*제약, "" = 전체*/, PhysicsTimelines {
  inertia, strength, damping, mass, wind, gravity, mix: Vec<Key<f32>>,   // 속성별 타임라인
  reset: Vec<f32>,                                                       // 리셋 시각
}>
```
- 기본값은 4.2 JSON 형식의 기본값과 동일하게 맞춘다(내보내기 1:1 매핑).
- 키가 제약 이름 `""`인 타임라인 = 모든 물리 제약 대상(global 타임라인).

#### 평가
- `ConstraintRef::Physics(i)`를 P0-1 스케줄에 추가. 스킨 소유 제약은 D3 규칙.
- 자체 구현한 스프링 적분(코드 이식 금지, §6.1-5). 모드별 동작
  - `None`: 아무것도 안 함(셋업·편집 확인).
  - `Reset`: 상태를 현재 본 포즈로 초기화.
  - `Update`: 경과 시간만큼 고정 스텝 적분 후 적용.
  - `Pose`: 마지막 상태를 다시 적용만(시간 진행 없음, 스크럽 시 사용).
- 관성은 본 월드 위치/회전 변화량 + 스켈레톤 이동량 기반, 바람·중력은 월드 축, 속도는 `limit`로 제한.
- 타임라인 `reset`을 지나면 그 스텝에서 `Reset`.

#### op
`add_physics(PhysicsConstraint)`, `set_physics(PhysicsPatch)`(order 포함), `remove_physics { name }`, `key_physics { animation, constraint, property, time, value, curve }`, `key_physics_reset { animation, constraint?, time }`, `set_physics_globals`. 본/제약 이름 변경·삭제 시 참조 정리, 스킨 `constraints` 갱신.
- 프리셋 op는 두지 않는다. 프리셋은 에디터/AI 가이드의 **값 묶음**이고 기록은 `set_physics` 하나.

#### CLI/HTTP/MCP
- `render`/`pose`: 애니메이션이 있으면 기본으로 `Pose::simulate(.., t, fps)` 사용(결정적). `--physics none|simulate`(기본 simulate), `--fps`(기본 60), `--move "x,y@t;..."`(스켈레톤 이동 경로, 선택).
- `skel2d_pose` 응답에 물리 제약별 상태(현재 오프셋·속도) 포함 → AI가 흔들림 크기를 수치로 확인·튜닝.
- 프레임 연속 렌더(`render --frames`)는 한 번의 시뮬레이션으로 이어서 계산(프레임마다 0초부터 재시뮬레이션 금지, 결과는 같아야 함 — 테스트).

#### 에디터
- 재생 중: `advance_playback`에서 `PhysicsState` 유지 + `Update`. 루프 경계에서 상태 유지.
- 스크럽/정지/키 편집: **시뮬레이션 캐시** `physics_cache { key: (문서 버전, 트랙, 스킨, fps), frames: Vec<PhysicsState> }` — 0초부터 프레임별 상태를 저장, 플레이헤드 이동 시 가장 가까운 이전 상태부터 이어 계산. 문서 변경 시 무효화. 스크럽 시 물리가 튀거나 리셋되지 않도록 한다.
- 셋업 모드: "물리 미리보기" 토글(기본 꺼짐). 켜면 본 드래그·루트 이동에 실시간 반응(리깅 중 흔들림 확인). 문서는 변경하지 않음.
- 상단 모드 스위치 옆 물리 컨트롤: 일시정지, 리셋(Shift+R), 끄기 토글.
- 아웃라이너 제약 탭에 물리 아이콘(`physics.png` → `icons.rs` 연결), 새 제약 대화상자(P0-2)에 물리 종류 추가(선택 본으로 미리 채움, 체인 선택 시 "본마다 하나씩 만들기" 체크 — 머리카락 체인에 일괄 생성).
- 인스펙터:
```
[물리 카드: hair_1_physics · 본 hair_1]
본        [hair_1 ▾]
영향      위치 X [■■■□□] Y [■■■□□]  회전 [■■■■□]  크기 [□□□□□]  셰어 [□□□□□]
프리셋    [머리카락 ▾]  (머리카락/천/꼬리/가슴/액세서리)
── 동작 ─────────────── (관성 1 · 강도 100 · 감쇠 1)
관성 [1.00] 🌐   강도 [100] 🌐   감쇠 [1.00] 🌐   질량 [1.00] 🌐
── 환경 ───────────────
바람 [0] 🌐   중력 [0] 🌐
── 고급 (접힘) ─────────
스텝 [60 Hz]  속도 제한 [5000]  믹스 [1.00] 🌐
[흔들어 보기]   (선택 본에 임펄스)
```
  - 🌐 = 전역 값 사용 토글. 애니메이트 모드에서는 각 값 앞에 `key_button`, 도프시트에 물리 행(키 없어도 표시) + 리셋 마커 행.
- 뷰포트: 물리 본에 작은 물결 표시, 선택 시 영향 축 화살표, 시뮬레이션 중 셋업 대비 오프셋을 흐린 고스트로.

#### 테스트
- 결정성: 같은 입력 `simulate` 결과 비트 단위 동일, 프레임 연속 렌더 = 개별 `simulate`.
- 강도 0·감쇠 1 정지 유지, 중력 > 0이면 아래로 처짐(부호), `limit` 속도 제한, `reset` 키 직후 셋업 복귀.
- 스크럽 캐시가 앞/뒤 이동 모두 `simulate`와 일치.
- 스킨 소유 물리 제약이 스킨 비활성 시 미적용.
- 외부 형식과 호환이 필요한 값은 형식 문서와 관찰한 동작을 기준으로 삼는다(타사 런타임 소스 이식 금지).

---

### M6. 마감
- `docs/AI_GUIDE.md`: 메쉬→가중치→스킨→물리 전체 흐름 예제(op JSON 순서대로), AnyOp 설명, 물리 튜닝 절차(`pose`로 오프셋 확인 → `set_physics` 조정).
- 예제 파일: `examples/` 중 하나에 스킨 2개 + 링크드 메쉬 + 머리카락 물리를 넣은 데모(CLI 렌더 회귀 테스트 기준 이미지로 사용).
- 매뉴얼(`docs/manual`) 해당 장 갱신, 한국어 번역 누락 테스트 통과.

---

## 4. 공통 규칙

### 4.1 불변 규칙 (v1에서 유지)
- 모든 문서 요소는 **이름으로 주소 지정**. 이름 변경·삭제 시 모든 참조(애니메이션, 스킨, 제약, 가중치, 디폼, 링크드 메쉬 부모)를 갱신.
- op 배치는 원자적(`apply` 실패 시 문서 불변, 성공 시 `validate` 통과). 에셋 op도 펼친 뒤 하나의 배치로 적용.
- 포맷 `skel2d/1` JSON. 새 필드는 `#[serde(default, skip_serializing_if = ...)]`. 구조 변경은 `skel2d_core::from_str` 마이그레이션 단계(예: `migrate_expressions`, 이번에 `migrate_deform_skins`, `migrate_skin_constraints`).
- 새 모델 필드/타입마다 `validate.rs` 검사 + op + 테스트 + `AI_GUIDE.md` 한 줄.
- UI 문자열은 영어 + `tr`/`tf!`, 한국어는 `i18n/ko_*.rs`(같은 영어 키 = 같은 번역, 테스트 검사).
- 색·크기는 `theme.rs` 상수/헬퍼만. 아이콘은 64×64 흰 글리프, `icons.rs`의 `Icon`과 `PNGS`를 같은 순서로.

### 4.2 UI/UX 원칙
- 선택 하나 = 인스펙터 하나: 헤더 카드 → 자주 쓰는 값 → 접는 그룹(헤더 오른쪽 요약).
- 애니메이트 모드에서 값 변경 = 키. 셋업 값을 몰래 바꾸는 입력 금지.
- 키 가능한 값 앞에 `key_button`(빨강=여기 키, 주황=변경 미키, 초록=보간, 흐린 초록=키 없음).
- 툴 옵션은 뷰포트 위 떠 있는 창. 옵션이 많은 일회성 작업은 대화상자 + 뷰포트 실시간 미리보기.
- 목록은 `tree_row`, 빈 목록은 `empty_state`.
- 모든 기능은 Ctrl+K 팔레트에서 이름으로 실행 가능, 모든 아이콘 버튼에 툴팁(기능 + 단축키), 색만으로 의미 전달 금지.
- 위험 작업은 확인 대화상자 또는 "Ctrl+Z로 취소" 상태바 안내.

### 4.3 작업 위생
- **커밋 보류(사용자 결정).** 큰 변경 전 `crates/`·`docs/`·`examples/`를 `%TEMP%`에 백업하고 경로를 진행 기록에 남긴다.
- 여러 줄 치환은 편집 도구로. `perl -pi`/`sed`에 `|` 구분자 + Rust 클로저 패턴 금지(파일 손상 사고 있었음).
- 단계 끝마다: `cargo test --workspace --release`, `cargo clippy --workspace --release -- -D warnings` 경고 0, 에디터 스크린샷 확인:
  `target/release/skel2d-editor FILE [--anim A --time T --select BONE --select-slot SLOT --tool NAME --skin K --tracks a,b] --size 1280x800 --screenshot out.png [--wait 1]`

### 4.4 완료 정의 (단계 공통)
- [ ] 모델 + serde 기본값(기존 파일 로드 불변) + 마이그레이션 테스트
- [ ] `validate.rs` 검사 + 잘못된 입력 테스트
- [ ] op + 이름 변경·삭제 참조 정리 테스트
- [ ] 포즈/렌더 반영 + 수치 테스트(호환 동작은 관찰한 기대값)
- [ ] CLI/HTTP/MCP 노출 + `AI_GUIDE.md`
- [ ] 에디터 UI(인스펙터·아웃라이너·도프시트·팔레트·단축키 도움말) + 한국어
- [ ] **에디터 전용 로직 없음**: 새 버튼이 만드는 op를 CLI로 실행해 같은 결과가 나오는지 확인
- [ ] 테스트·Clippy 통과, 셋업/애니메이트 스크린샷 확인, 진행 기록 갱신

---

## 5. 백로그 (이번 범위 밖, 설계는 v1 문서 참고)
| v1 단계 | 내용 | 비고 |
|---|---|---|
| P2 | IK softness/stretch/compress/uniform, IK 키 일반화 | 물리 이후 우선순위 1 |
| P3 | 트랜스폼 제약 | D3 덕분에 스킨 연동은 바로 가능 |
| P5 | 블렌드 모드 + 다크 틴트, wgpu 슬롯 파이프라인 | 사용자 결정: wgpu로 전환 |
| P6 | 커브 일반화, 키 전체, X/Y 분리, 도프시트 행, 그래프 에디터, 드로우 오더 타임라인, 이벤트 | 애니메이션 복제는 완료 |
| P7 | 포인트/바운딩 박스/클리핑/패스 어태치먼트 + 패스 제약 | 링크드 메쉬는 M4로 앞당김 |
| P9 | 외부 JSON 4.2 형식, 아틀라스, PNG 시퀀스/GIF/비디오 | Skel2D 전용 개념 처리 방식은 착수 전 사용자 확인 |

---

## 6. 결정 사항

### 6.1 사용자 결정 (유지)
1. 뷰포트는 wgpu 커스텀 파이프라인으로 전환한다(P5, 이번 범위 밖).
2. 애니메이션의 스킨 전환 키는 도입하지 않는다.
3. 외부 JSON 내보내기의 Skel2D 전용 개념 처리는 보류, P9 착수 전 확인.
4. 커밋·브랜치 전략은 보류. 지시 전까지 커밋 금지.
5. 타사 런타임 코드를 이식하지 않는다. 형식과 동작을 맞추되 구현은 직접 한다. 소스를 옮겨 쓰면 해당 런타임의 라이선스 조건을 받는다(사용자마다 상용 에디터 라이선스 필요). 이 저장소에 있던 소스 기반 구현은 재구현한다.

### 6.2 이번 문서에서 정한 것 (착수 전 이의 있으면 사용자 확인)
- 이번 범위 = 메쉬·가중치·스킨·물리. IK 확장 등은 백로그.
- 이미지 필요 op는 `skel2d-rig`의 에셋 op로 두고 코어 op로 펼친다(D1).
- 디폼 키를 스킨별로 마이그레이션(D2), 스킨 제약 일반화(D3), `default` 스킨 이름 예약.
- 물리 CLI 렌더 기본값은 시뮬레이션 켬(`--physics none`으로 끔).

### 6.3 의도적인 설계 차이 (유지할 것)
- 어태치먼트 이름 변경을 현재 스킨만(`rename_attachment`) / 모든 스킨의 같은 플레이스홀더(`rename_placeholder`)로 명시적으로 구분한다.
- 다중 스킨 동시 미리보기, 미리보기 중 편집이 스킨으로 자동 전환, 스킨 간 누락 플레이스홀더 표시.
- 키 `reset` = 아래 트랙 값.
- 메쉬 재생성·정점 편집·UV 리셋이 가중치·디폼 키를 보존. 가중치 위치 기반 전송·미러. 잠금 본 일관 적용.
- 트레이스 옵션 실시간 미리보기.
- Scale 툴 Alt+드래그 = 셰어.
- 미키 변경을 조용히 버리지 않고 상태바로 알림.
- 물리 스크럽 캐시, 셋업 물리 미리보기, 체인 일괄 생성, 프리셋, 흔들어 보기.

---

## 7. 진행 기록
(각 단계 완료 시 날짜·구현 요약·검증 결과·백업 경로를 여기에 추가. v1의 P0~P1 기록은 `NEXT_PHASE_v1.md` 참고.)

### 2026-10-30 — M0 완료: 스킨별 디폼과 어태치먼트 이름 변경

- 구현: `Animation.deform[skin][slot][attachment]`, `migrate_deform_skins`, `Document::resolve_attachment_from`을 추가했다. 기본 스킨 경로는 `default`이며 사용자 스킨 이름으로는 금지한다. 기존 슬롯 디폼은 로드 시 기본 스킨으로 옮긴다. 구형/신형 디폼 경로가 충돌하는 혼합 파일은 덮어쓰지 않고 오류로 반환한다.
- op: `key_deform`·`remove_key`·`clear_timeline`에 선택적 `skin`을 추가했다. `rename_attachment`는 지정 스킨만, `rename_placeholder`는 모든 스킨의 같은 플레이스홀더를 변경한다. 공유 셋업 표시 이름·어태치먼트 키와 대상 디폼 경로도 갱신한다. 스킨·슬롯·본·어태치먼트 이름 변경/삭제, 메쉬 교체의 디폼 참조 정리도 연결했다.
- 에디터: 어태치먼트 행 선택과 F2, 슬롯 인스펙터 이름 변경 버튼, 스킨 인스펙터 이름 변경 메뉴, 모든 스킨 변경 체크박스, Ctrl+K 명령, F1 도움말과 한국어 번역을 추가했다. 정점 드래그는 실제 표시 중인 스킨에 디폼 키를 기록하며 도프시트의 키 표시·이동·복사·삭제는 스킨 경로를 보존한다. `AI_GUIDE.md`와 매뉴얼도 갱신했다.

§4.4 완료 정의 확인:

- [x] 모델·serde 기본값·마이그레이션: 기존 파일 로드/왕복 직렬화, 구형 디폼 마이그레이션 후 포즈 동일성, 충돌 경로 거부를 검증했다. 기존 `examples/**/*.skel.json`도 전부 로드·검증했다.
- [x] `validate.rs`·잘못된 입력: 스킨/슬롯/어태치먼트 경로, 정점 수, 음수 시각, 비유한 오프셋, 예약 이름과 빈 이름을 검사했다. 잘못된 op 배치는 원본 문서를 바꾸지 않는다.
- [x] op·참조 정리: 현재 스킨/전체 스킨 이름 변경, 충돌 거부, 스킨·슬롯 이름 변경과 삭제, 본 삭제, 어태치먼트 삭제/교체를 테스트했다. 다른 스킨의 디폼은 유지한다.
- [x] 포즈·렌더·수치: 기본 메쉬 4정점과 `armor` 메쉬 9정점의 독립 디폼, 보간·트랙 alpha·스킨 fallback을 기대 수치로 검사했다. CLI 렌더 두 장에서도 서로 다른 변형을 확인했다.
- [x] CLI/HTTP/MCP·가이드: 공통 코어 op 파싱 경로로 자동 노출한다. 실제 CLI `apply`, HTTP `POST /ops`, MCP `skel2d_apply_ops`에 동일한 9개 op를 적용해 저장 파일이 바이트 단위로 일치함을 확인했다. HTTP/MCP op 스키마도 확인했다.
- [x] 에디터 UI·한국어: 인스펙터·아웃라이너·도프시트·팔레트·도움말을 연결했다. 한국어 셋업/애니메이트 스크린샷과 번역 테이블 일치 테스트를 확인했다.
- [x] 에디터 전용 로직 없음: 이름 변경 폼이 생성한 op를 JSON 직렬화·재파싱해 CLI와 같은 코어 API로 실행한 결과가 일치한다. 도프시트 이동·복사·삭제와 정점 드래그 op의 스킨 보존 테스트도 통과했다.
- [x] 테스트·Clippy·스크린샷·기록: `cargo test --workspace --release` 120개 통과, `cargo clippy --workspace --release -- -D warnings` 경고 0, `cargo build --workspace --release` 성공. 최종 실행 파일로 셋업/애니메이트 스크린샷을 다시 생성하고 열어 확인했다. 실제 F2 키 입력·드래그 제스처의 수동 검증은 하지 않았으며, 폼/op 테스트와 화면 검증을 수행했다.

검증 환경: 최초 병렬 릴리스 테스트 빌드에서 rustc가 `STATUS_ACCESS_VIOLATION`으로 종료했다. `CARGO_BUILD_JOBS=1`로 최종 전체 테스트·Clippy·실행 파일 빌드를 모두 재실행해 성공했다.

백업: `C:\Users\dnwls\AppData\Local\Temp\Skel2D-M0-20261030-172045` (큰 변경 전 `crates/`·`docs/`·`examples/` 전체 복사).

검증 산출물: 같은 백업 폴더의 `qa/`에 `tests.log`, `clippy.log`, `build.log`, `verification.json`, `cli.skel.json`, `http.skel.json`, `mcp.skel.json`, `ops.json`, `default.png`, `armor.png`, `setup.png`, `animate.png`를 남겼다. 실제 호출 재현 스크립트는 백업 폴더의 `qa.mjs`이다.

§6.2 확정 사항은 그대로 유지했다. 커밋하지 않았고, M1은 시작하지 않았다.

### 2026-10-30 — M1 완료: 메쉬 op 승격과 헐·제약 엣지

- 모델: `Mesh.hull`·`Mesh.edges`를 추가했다. 기본값 0/빈 배열은 직렬화에서 생략하며, 기존 파일을 로드할 때 정점 순서를 바꾸지 않는다. 단일 외곽은 재삼각화 시 앞쪽 정점에 시계 방향으로 정리한다. 구멍은 삼각형 경계로 보존한다. 분리된 기존 메쉬는 하나의 외곽으로 표현할 수 없으므로 `hull=0`을 유지하고 각 섬과 빈 공간을 보존한다.
- 코어: `mesh.rs`에 Spade 제약 들로네 삼각화, 경계 추론·클리핑, 정점 추가·삭제·생성, UV 리셋과 UV/가중치 보간을 옮겼다. 에디터의 Lawson flip·ear clip 구현은 제거했다. 경계 정점 삭제 시 살아남은 삼각형 팬의 내부 정점을 새 경계로 승격하므로 정점을 잃거나 새 헐 밖에 남겨 두지 않는다.
- op: `add_mesh_vertices`, `remove_mesh_vertices`, `set_mesh_edges`, `generate_mesh_vertices`, `reset_mesh`를 추가했다. 모두 선택적 `skin`을 받는다. 출력 정점별 원본 인덱스·보간 계수로 해당 스킨의 모든 디폼 키를 재매핑한다. 제약 엣지의 끝점도 재매핑하고, 엣지 위 정점 추가 시 분할하며, 정점 삭제 시 그 정점에 닿는 제약을 제거한다.
- 에디터: 수정/생성/삭제/엣지 모드, 우클릭 모드 순환, 정점 간 드래그로 제약 추가, 제약 선택·삭제, 두 UV 리셋 버튼, 팔레트·도움말·한국어를 연결했다. 헐은 굵은 청록색, 제약은 주황색, 일반 삼각형은 흐린 선으로 표시한다. 토폴로지 변경은 셋업 전용이며 변경 후 Undo 안내를 표시한다. 토폴로지 변경·Undo/Redo 후 이전 정점/엣지 선택은 비운다.
- 통합: 셋업 본 편집으로 메쉬를 함께 이동할 때 반사 등으로 바뀐 헐 방향도 공통 `set_mesh_edges` op로 정규화하고 디폼 인덱스를 갱신한다. 정규화 실패는 에디터 변경 실패로 반환하므로 잘못된 문서를 저장하지 않는다. `AI_GUIDE.md`와 매뉴얼도 갱신했다.

§4.4 완료 정의 확인:

- [x] 모델·serde 기본값·이전 파일 호환: 메타데이터 없는 파일의 정점 순서·포즈·왕복 직렬화가 동일함을 테스트했다. 별도 포맷 마이그레이션은 필요하지 않다. 기존 예제 문서 5개도 모두 로드·검증했다.
- [x] `validate.rs`·잘못된 입력: 헐 범위·시계 방향·삼각형 경계, 제약 끝점·중복·삼각형 엣지 여부·교차를 검사한다. 외부/중복/비유한 점, 잘못된 삭제 인덱스, 경계 붕괴, 잘못된 생성 인자, 퇴화 UV의 실패와 배치 원자성을 테스트했다.
- [x] op·참조 정리: 추가·삭제·생성·재삼각화 후 디폼 인덱스와 엣지 끝점을 확인했다. 스킨별 독립성, 변경 후 스킨·슬롯·플레이스홀더 이름 변경과 어태치먼트 삭제의 참조 정리도 통과했다.
- [x] 포즈·렌더·수치: 오목한 헐 밖 삼각형 없음, 구멍/분리된 섬 보존, 제약 유지·분할, UV/가중치/디폼 보간, 경계 삭제, 본 반사, 비아핀 편집의 두 UV 리셋을 수치로 확인했다. 텍스처 재삼각화 전후 렌더는 채널당 최대 차이 2/255로 허용값 3/255 이내였다.
- [x] CLI/HTTP/MCP·가이드: 기존 공통 op 경로로 5종을 노출했다. 실제 CLI `apply`, HTTP `POST /ops`, MCP `skel2d_apply_ops`에 동일한 8개 op를 적용해 저장 파일이 바이트 단위로 일치했다. 잘못된 op는 세 경로 모두 거부했고, 기존 저장 결과가 유지됐다. HTTP/MCP 스키마와 가이드도 확인했다.
- [x] 에디터 UI·한국어: 인스펙터의 정점/삼각형 수와 메쉬 툴 창, 경계·제약 표시, 도프시트 디폼 유지, 팔레트·F1 도움말·한국어를 연결했다. 실제 Windows 에디터에서 우클릭 순환, 생성 클릭, 엣지 드래그·선택, Generate, 두 UV 리셋, 한국어 모드 검색·실행을 확인했다. 최종 릴리스 셋업/애니메이트 스크린샷도 열어 확인했다.
- [x] 에디터 전용 새 로직 없음: 실제 UI로 저장한 정점 추가·엣지 드래그·Generate·두 리셋 결과를 같은 op의 CLI 실행 결과와 비교해 UV·가중치·모든 디폼 키까지 일치했다. 삭제 op는 코어 수치 테스트와 CLI/HTTP/MCP 배치로 검증했다. 삭제 제스처 자체는 Windows UI에서 수동 실행하지 않았다.
- [x] 테스트·Clippy·스크린샷·기록: `cargo test --workspace --release` 133개 통과, `cargo clippy --workspace --release -- -D warnings` 경고 0, `cargo build --workspace --release` 성공. 최종 검증은 `CARGO_BUILD_JOBS=2`로 실행했다.

구현 규약: 토폴로지 op 뒤 인덱스가 바뀔 수 있으므로 다음 op의 인덱스는 갱신된 문서를 기준으로 한다. UV 리셋은 현재 데이터의 아핀 최소제곱 배치를 사용하며 원본 이미지 크기나 편집 전 프레임을 별도로 읽거나 저장하지 않는다. 가중치·디폼 오프셋은 지우지 않는다.

추가 검사 메모: 필수 Clippy 명령은 통과했다. 별도로 확대한 `cargo clippy --workspace --all-targets -- -D warnings`는 기존 `crates/skel2d-core/tests/shear_inherit.rs`의 `excessive_precision` 9건으로 실패했다. 이 기존 기준 수치 파일은 M1에서 변경하지 않았다.

백업: `C:\Users\dnwls\AppData\Local\Temp\Skel2D-M1-20261030-175817` (큰 변경 전 `crates/`·`docs/`·`examples/` 전체 복사).

검증 산출물: 같은 백업 폴더의 `qa/`에 `tests.log`, `clippy.log`, `build.log`, `verification.json`, `ui-verification.json`, API별 문서·op JSON, UI/CLI 비교 문서, `before.png`, `cli.png`, `armor.png`, `setup.png`, `animate.png`, `animate-mesh-controls.png`를 남겼다. 실제 API 호출·렌더 비교 재현 스크립트는 백업 폴더의 `qa.mjs`이다.

§6.2 확정 사항은 그대로 유지했다. 커밋하지 않았고, M2는 시작하지 않았다.

### 2026-10-30 — M2 완료: 에셋 op와 트레이스 실시간 미리보기

- 공통 경로: `AssetOp::{TraceMesh, ImageWeights}`, 명시적 태그 분기의 `AnyOp`, `parse_ops`, `expand`, `apply_any`를 rig에 추가했다. 코어의 `apply_expanded`는 혼합 입력을 현재 임시 문서 기준으로 순서대로 펼치고 최종 문서만 검증·반환한다. 코어는 이미지를 읽지 않는다. CLI·HTTP·MCP·에디터가 같은 경로와 통합 스키마를 사용하며, 응답에 입력/확장 op 수·정점 수·삼각형 수·헐·가중치 본 목록을 포함한다.
- 이미지 처리: 에디터의 트레이스·UV 아핀 배치 추정·지오데식 가중치 로직을 rig로 옮겼다. 알파·패딩·디테일·오목 윤곽 허용 오차·내부 정점 none/grid/auto·간격·가중치 유지를 연결했다. 기존 자동 리깅 진입점의 기본 동작과 격자 fallback을 유지한다. 재트레이스는 제약 엣지 끝점을 새 표면에 투영하고, 해당 소유 스킨의 디폼 키를 원래 삼각형에서 보간해 곡선과 함께 보존한다.
- 에디터: 리전 생성과 메쉬 재생성에 같은 대화상자를 사용한다. 파라미터 변경은 즉시 `app.preview`에 반영하고, 취소는 원본을 복원한다. 적용은 미리보기 때 확정한 코어 op를 한 번의 Undo로 기록한다. Undo/Redo에 스냅샷과 펼쳐진 op를 보관해 이미지를 다시 읽지 않는다. 이미지 가중치 버튼도 AssetOp를 만들며 선택 정점만 변경할 수 있다. 인스펙터·Ctrl+K·Enter/Escape·한국어를 연결했고 에셋 편집은 셋업 포즈로 제한했다.
- 자료: `AI_GUIDE.md`와 매뉴얼을 갱신했다. 반투명 테두리 테스트 이미지 `examples/arm/trace.png`와 재현 배치 `trace.ops.json`을 추가했다. 기존 `arm.skel.json`은 바꾸지 않았다.

§4.4 완료 정의 확인:

- [x] 모델·serde 기본값·기존 파일 호환: 문서 형식은 바꾸지 않아 새 마이그레이션은 필요하지 않다. AssetOp 기본값·왕복 직렬화·알 수 없는 필드 거부·태그 비충돌을 테스트했다. 기존 마이그레이션 테스트와 예제 문서 5개의 로드·검증도 통과했다.
- [x] `validate.rs`·잘못된 입력: 최종 문서는 기존 코어 검증을 통과해야 한다. 에셋 계층의 비유한 값·범위·본/정점 선택·투명/누락 이미지·배치 오류를 검사하며, 실패한 혼합 배치와 잘못된 미리보기가 원본을 바꾸지 않는지 확인했다.
- [x] op·참조 정리: 정확한 스킨의 이름 변경과 디폼 경로 이동, 다른 스킨의 독립성, 이름 변경 뒤 삭제 참조 정리를 테스트했다. 제약 엣지·가중치·디폼 곡선 보존, 선택 정점 이미지 가중치, 이미지 없는 코어 op 재생도 통과했다.
- [x] 포즈·렌더·수치: arm 이미지의 임계값 0.05/0.5 및 패딩 0/2 경계 좌표, 세 내부 정점 방식의 결정성, 구멍 내부 삼각형 제외, 선형 디폼 보간을 수치로 확인했다. 기존 다리의 반대쪽 본 혼입 회귀와 빈 공간을 도는 지오데식 테스트도 통과했다. 트레이스/디폼 렌더와 최종 에디터 셋업/애니메이트 스크린샷을 열어 확인했다. 자체 트레이스 알고리즘이므로 외부 도구의 동일 토폴로지 수치를 기준으로 삼지는 않았다.
- [x] CLI/HTTP/MCP·가이드: 실제 세 경로에 같은 5개 혼합 op를 적용한 파일이 바이트 단위로 일치했다. 8개 코어 op로 펼쳐졌고 기본 메쉬 49정점/92삼각형과 armor 메쉬 37정점/52삼각형의 요약·본 목록도 일치했다. dry-run 불변, 실패 롤백, 이미지 파일을 옮긴 뒤 HTTP/MCP Undo, 통합 스키마, arm 예제 배치도 확인했다.
- [x] 에디터 UI·한국어: 실제 Windows 한국어 대화상자의 auto 미리보기 37정점, none 미리보기 4정점, 취소 후 원본 27정점 복원을 확인했다. 적용 4정점, Undo 27정점, Redo 4정점도 확인했다. egui 대화상자 테스트는 Enter/Escape, 오류 시 복원, 애니메이트 모드 거부, 이미지 경로 변경 뒤 캐시 적용을 검사한다. 기존 아웃라이너·도프시트의 스킨별 디폼 경로와 키 곡선은 유지하며, 한국어 번역 일치 테스트도 통과했다.
- [x] 에디터 전용 새 로직 없음: 이미지 연산은 rig에 있고 에디터 import 함수는 AssetOp를 생성한다. 실제 UI에서 저장한 none 트레이스와 같은 AssetOp의 CLI 결과가 바이트 단위로 일치했다. 대화상자 op의 JSON 왕복 및 공통 경로 결과 비교 테스트도 통과했다.
- [x] 테스트·Clippy·스크린샷·기록: 최종 `cargo test --workspace --release` 142개 통과, `cargo clippy --workspace --release -- -D warnings` 경고 0, `cargo build --workspace --release` 성공. `CARGO_BUILD_JOBS=2`로 실행했다. 최종 실행 파일의 1280×800 셋업/애니메이트 스크린샷을 생성·확인했고 이 기록을 남겼다.

파라미터 규약: 패딩은 원본 픽셀의 정수 부분을 쓰는 사각 이웃 확장이며 이미지 범위로 제한된다. `concavity`는 픽셀 단위 윤곽 단순화 허용 오차다. Density 프리셋은 detail 0/0.5/1과 자동 spacing으로 매핑하고 다른 사용자 파라미터를 유지한다. 메쉬 배치는 UV에서 셋업 위치로의 아핀 최소제곱 추정을 사용한다. 세부 규약과 한도는 `AI_GUIDE.md`에 기록했다.

검증 메모: 최초 Clippy에서 커진 대화상자 enum과 중첩 조건문을 지적해 MeshOptions를 Box로 옮기고 조건을 합쳤다. 이후 최종 전체 테스트·Clippy·빌드를 다시 실행해 모두 통과했다.

백업: `C:\Users\dnwls\AppData\Local\Temp\Skel2D-M2-20261030-214825` (큰 변경 전 `crates/`·`docs/`·`examples/` 전체 복사).

검증 산출물: 같은 백업 폴더의 `qa/`에 `tests.log`, `clippy.log`, `build.log`, `verification.json`, `ui-verification.json`, `ops.json`, API별 문서, UI/CLI 비교 문서, `arm-sample.skel.json`, `traced.png`, `deformed.png`, `setup.png`, `animate.png`를 남겼다. 재현 스크립트는 백업 폴더의 `check.mjs`, `qa.mjs`, `screenshots.mjs`다. 실제 UI 조작 결과는 별도로 기록했고 `qa.mjs` 재실행은 기존 UI 파일을 덮어쓰지 않는다.

§6.2 확정 사항은 그대로 유지했다. 커밋하지 않았고 M3는 시작하지 않았다.

### 2026-10-31 — M3 완료: 가중치 op·바인딩 본 관리·가중치 UX

- 코어: `smooth_weights`, `prune_weights`, `swap_weights`, `set_bone_weight`, `bind_bones`, `normalize_weights`를 추가했다. 가중치·브러시·복사/붙여넣기·재분배 알고리즘은 `mesh.rs`로 옮겼다. 선택 정점, 스킨, 수치 입력의 set/add/scale, 스무스 반복 횟수, 프루닝 임계값과 최대 본 수를 지원한다. 생략한 정점 범위는 전체, 명시적 빈 배열은 변경 없음이다. 가중치 결과는 정규화하고 네 자리 반올림의 잔여 오차를 보정한다. 기존 자동 가중치가 저장하던 명시적 슬롯 본 100% 형식도 보존한다.
- 잠금: 요청의 `locked_bones`와 에디터 세션 상태로만 관리한다. 브러시·붙여넣기·수치·스무스·프루닝·교환·거리/이미지 자동 가중치·전송·미러에 적용한다. 잠금 값은 유지하고 남은 몫만 분배한다. 기존 프루닝이 잠긴 5%를 재정규화로 늘리던 회귀도 고쳤다. 잠금이 최대 본 수/임계값보다 우선하며 잔여 몫을 받을 비잠금 본 하나는 유지한다. 명시적 본 제거와 전체 정규화는 해당 op의 전체 대상에 적용한다.
- 바인딩: 선택적 `Mesh.bones`에 영향도 0인 본도 저장한다. 빈 목록은 직렬화에서 생략하고 기존 파일은 정점 가중치와 슬롯 본에서 목록을 추론한다. 제거 시 auto는 살아남은 영향도 비례, nearest는 셋업 본 선분 거리, parent는 남은 바인딩 조상을 우선한다. 비례 분배할 영향도나 바인딩 조상이 없으면 가장 가까운 남은 본을 사용한다. 마지막 바인딩 본 제거는 거부한다. 본 이름 변경·삭제는 기본/스킨 메쉬의 목록과 가중치를 정리한다. 세션 잠금·복사한 가중치도 이름 변경과 Undo/Redo를 따라간다.
- 공통 경로: M2의 명시적 태그 `AnyOp`와 `apply_any`를 유지했다. 에디터 버튼과 대화상자는 같은 코어/에셋 op를 생성하고 CLI·HTTP·MCP가 그대로 실행한다. 응답에 `pruned_influences`와 메쉬 요약을 제공한다. 이미지 가중치는 잠금을 반영한 `set_vertices`로 펼치므로 이미지 없이도 Undo와 재생이 가능하다.
- 에디터: 가중치 창에 바인딩 본 추가/제거, 트리 행, 선택 정점(없으면 전체)의 평균 영향도 막대, 수치 입력, 잠금, 스무스·프루닝·교환·정규화, 거리/이미지 자동 가중치, 미러·전송을 연결했다. 프루닝·미러·전송·본 제거는 옵션과 변경 정점/제거 영향 수를 표시하는 실시간 미리보기 대화상자를 사용한다. 취소는 원본 복원, 적용은 한 번의 Undo다. 브러시도 연속 미리보기 후 놓을 때 `set_vertices` 하나로 기록한다. 가중치 변경은 셋업 전용이며 애니메이트 모드에서는 비활성이다.
- 포즈 테스트: 문서와 바인드 포즈를 바꾸지 않는 임시 평가 트랙을 사용한다. Alt+드래그로 임시 회전하고 놓으면 복귀한다. 테스트 각도를 정하고 T를 누른 동안 변형된 포즈에서 칠할 수 있으며 T를 놓으면 셋업으로 복귀한다. Ctrl+K 명령·F1 도움말·한국어·`AI_GUIDE.md`·매뉴얼과 가중치 스크린샷을 갱신했다. 실제 UI 검증에서 발견한 기존 Ctrl+Shift+Z의 Undo 오인식도 더 구체적인 Redo 단축키를 먼저 소비하도록 고쳤다.

§4.4 완료 정의 확인:

- [x] 모델·serde 기본값·기존 파일 호환: `Mesh.bones`의 생략/추론 및 영향도 0인 바인딩 저장·왕복을 검증했다. 별도 마이그레이션은 필요하지 않다. 기존 마이그레이션 테스트와 예제 문서 5개의 로드·검증도 통과했다. 예제 파일은 백업본과 모두 동일하다.
- [x] `validate.rs`·잘못된 입력: 바인딩 본의 존재·중복을 검사한다. op의 수치 범위, 정점 범위·중복, 없는 본/스킨, 마지막 본 제거, add/remove 중복과 실패 배치의 원자성을 테스트했다.
- [x] op·참조 정리: 여섯 op의 기대 수치, 선택 정점 불변 범위, 스킨 독립성, 바인딩 추가·세 재분배 방식·제거 후 영향 0, 본 이름 변경·삭제와 세션 이름 변경/Undo/Redo를 확인했다. 배치 전후 위치·UV·삼각형과 애니메이션/디폼 키는 보존한다.
- [x] 포즈·렌더·수치: 잠금 값 불변, 합 1, 프루닝 잠금 우선, 스무스 이웃 평균, set/add/scale, 교환, 정규화, 임시 90도 회전의 스키닝 좌표를 기대값과 비교했다. 포즈 테스트 해제 시 복원·문서 불변도 검사했다. CLI 렌더와 최종 셋업/애니메이트 스크린샷을 열어 확인했다. 가중치 편집 자체의 기대값은 자체 수학 규약을 사용한다.
- [x] CLI/HTTP/MCP·가이드: 실제 세 경로에 같은 13개 혼합 op를 적용한 저장 파일이 바이트 단위로 일치했다. 프루닝 제거 수 3, 기본/armor 메쉬 각각 27정점·32삼각형, 본 목록 요약도 일치했다. dry-run 불변, 실패 롤백, 이미지 없는 Undo, 통합 스키마와 기존 예제 5개 검증을 통과했다.
- [x] 에디터 UI·한국어: 실제 Windows 한국어 창에서 영향도 0인 goal 추가, 평균 막대, 잠금/수치/포즈 옵션, 한국어 팔레트 검색, 프루닝과 스무스 대화상자를 확인했다. 스무스 미리보기는 27정점 변경을 표시했고 취소 후 원본이 바이트 단위로 유지됐다. 최종 실행 파일의 Ctrl+Z·Ctrl+Shift+Z와 저장 결과도 확인했다. 인스펙터·아웃라이너에서 도구로 진입하며 도프시트의 기존 디폼 경로를 유지한다. 한국어 번역 일치 테스트도 통과했다.
- [x] 에디터 전용 새 로직 없음: 실제 UI로 저장한 goal 바인딩·최대 본 수 1 프루닝·스무스 결과가 같은 op의 CLI 결과와 바이트 단위로 일치했다. 대화상자 10종의 op JSON 왕복·공통 경로·미리보기·취소·적용·Undo/Redo를 테스트했다. T 유지/해제와 브러시 한 번 기록은 egui 입력/수치 테스트로 검증했으며, 두 제스처 자체를 Windows에서 수동 수행했다고 주장하지 않는다.
- [x] 테스트·Clippy·스크린샷·기록: 최종 `cargo test --workspace --release` 152개 통과, `cargo clippy --workspace --release -- -D warnings` 경고 0, `cargo build --workspace --release` 성공. `CARGO_BUILD_JOBS=2`를 사용했다. 최종 1280×800 셋업/애니메이트 스크린샷을 생성·확인하고 이 기록을 남겼다.

검증 메모: 최초 회귀에서 자동 가중치의 명시적 슬롯 본 100% 저장이 빈 목록으로 바뀌는 차이를 발견해 보존했다. egui 테스트 API와 Clippy의 대입 공백 지적을 수정했다. 실제 Ctrl+Shift+Z 문제에는 입력 회귀 테스트를 추가했다. 중단 전 실행 중이던 검증용 에디터가 exe를 점유해 마지막 빌드가 실패했으나, 재개 후 해당 창을 닫고 최종 빌드를 성공시켰다. 이후 최종 바이너리로 실제 단축키와 스크린샷을 다시 확인했다.

백업: `C:\Users\dnwls\AppData\Local\Temp\Skel2D-M3-20261030-223343` (큰 변경 전 `crates/`·`docs/`·`examples/` 전체 복사).

검증 산출물: 같은 백업 폴더의 `qa/`에 `tests.log`, `clippy.log`, `build.log`, `checks.json`, `verification.json`, `ui-verification.json`, `ops.json`, API별 문서, 바인딩/프루닝/스무스 UI·CLI 비교 문서, `traced.png`, `deformed.png`, `setup.png`, `animate.png`를 남겼다. 재현 스크립트는 백업 폴더의 `checks.mjs`, `qa.mjs`, `screenshots.mjs`다. 실제 네이티브 UI 결과는 별도 기록이며 `qa.mjs`는 기존 UI 파일을 덮어쓰지 않는다.

§6.2 확정 사항은 그대로 유지했다. 기존 미커밋 작업을 보존했고 커밋·브랜치를 생성하지 않았다. M3에서 멈추며 M4는 시작하지 않았다.

### 2026-10-31 — M4 완료: 스킨 보강

- 링크드 메쉬: `Attachment::LinkedMesh`에 부모 메쉬·부모 스킨·이미지 덮어쓰기·`timelines`를 추가했다. 부모의 정점·삼각형·헐·가중치를 해석 시 사용하고, `timelines=true`면 부모 경로의 디폼 키를 사용한다. 부모 정점 수 변경에 따른 자식 고유 디폼 크기도 맞춘다. 링크 체인과 순환은 거부한다.
- op와 참조 정리: `add_linked_mesh`를 추가했다. 부모 어태치먼트·스킨 삭제는 `on_parent_removed`를 요구하며 기본값은 오류, `freeze`는 독립 메쉬 및 필요한 디폼 키로 굳히고 `delete`는 자식을 함께 지운다. 부모·스킨 이름 변경은 링크 참조를 갱신한다. 링크 자식의 공유 디폼 키 생성·삭제는 부모 경로로 보낸다.
- 스킨 제약: `Skin.ik`를 `Skin.constraints`로 일반화했다. 기존 `ik` 파일은 로드 시 마이그레이션하고 중복 필드 충돌은 거부한다. 기존 `set_skin`의 `ik` 입력은 호환 별칭으로 받는다. 현재 제약 종류인 IK의 활성화·이름 변경·삭제 참조를 검증한다.
- 에디터: 링크드 메쉬 생성 대화상자, 부모로 이동, 기하 편집 비활성 안내, 부모 삭제 정책 대화상자, 아웃라이너 표시를 연결했다. 스킨 인스펙터의 슬롯→플레이스홀더→어태치먼트 트리, 누락 항목 흐림, 제약 아이콘 목록, 스킨 비교 팔레트 명령·상태바·Ctrl+K·F1 및 한국어 문구를 추가했다. 에디터 변경은 `AnyOp`/`apply_any` 공통 경로를 사용한다.
- API·문서: CLI `pose --skin`, HTTP `/pose?skins=`, MCP `skel2d_pose`가 활성 스킨별 실제 어태치먼트와 출처 스킨을 반환한다. `info`도 스킨별 해석 결과를 포함한다. `AI_GUIDE.md`와 매뉴얼을 갱신했다.

§4.4 완료 정의 확인:

- [x] 모델·serde 기본값·기존 파일 로드·`ik`→`constraints` 마이그레이션: 기존 예제 문서 5개 검증, 기본값 직렬화 및 충돌 입력 테스트 통과.
- [x] `validate.rs`·잘못된 입력: 부모 부재·링크 체인·스킨/제약 오류 및 실패 배치 원자성 테스트 통과.
- [x] op·이름 변경·삭제 참조 정리: 부모/스킨 이름 변경, 부모/스킨 삭제의 오류·굳히기·동반 삭제, 디폼 경로 테스트 통과.
- [x] 포즈·렌더·수치: 부모 정점 편집 반영, `timelines` 켬/끔 디폼, 스킨별 IK 활성화 및 링크드/부모 렌더를 확인했다.
- [x] CLI·HTTP·MCP·`AI_GUIDE.md`: 동일 7개 op 배치 결과를 바이트 단위로 비교했다. dry-run 불변, 실패 롤백, Undo, pose/info/schema 응답을 확인했다.
- [x] 에디터 UI·한국어: 인스펙터·아웃라이너·도프시트·팔레트·단축키 도움말을 연결하고 번역 표 테스트를 통과했다. 1280×800 셋업·애니메이트 스크린샷을 확인했다.
- [x] 에디터 전용 로직 없음: 링크 생성 UI가 만든 op를 JSON으로 직렬화해 CLI와 같은 `parse_ops`/`apply_any` 경로로 실행한 결과·Undo를 테스트했고 기존 `AnyOp`/`apply_any` 구조를 유지했다.
- [x] 테스트·Clippy·스크린샷·기록: `cargo test --workspace --release` 159개 통과, `cargo clippy --workspace --release -- -D warnings` 경고 0, `cargo build --workspace --release` 성공. 최종 바이너리로 API 검증과 스크린샷을 다시 생성했다.

백업: `C:\Users\dnwls\AppData\Local\Temp\Skel2D-M4-20261031-092133` (변경 전 `crates/`·`docs/`·`examples/` 전체 복사). `examples/` 62개 파일은 백업과 해시가 같다.

검증 산출물: 같은 백업 폴더의 `qa/verification.json`, `qa/ops.json`, API별 문서, `linked.png`, `parent.png`, `setup.png`, `animate.png`. 재현 스크립트는 `qa.mjs`. 기존 미커밋 변경은 보존했고 커밋·브랜치를 만들지 않았다. M5는 시작하지 않는다.

### 2026-10-31 — M4 검수 결함 수정

- 부모 토폴로지 op와 재트레이스가 `timelines=false` 링크드 자식의 고유 디폼 키도 재매핑한다. 이전에는 정점 수 불일치로 배치가 거부됐다.
- `rename_attachment`는 다른 소유자가 옛 이름을 계속 채우는 동안 슬롯 셋업과 어태치먼트 키를 유지하고, 마지막 소유자가 바뀔 때만 함께 옮긴다. M0의 "셋업과 키를 항상 갱신" 설계와 테스트를 변경했다. 이전 동작은 다른 스킨의 표시를 깨뜨렸다.
- `timelines=true` 링크드 메쉬는 고유 키를 평가에서 무시하고, 해당 설정 시 삭제한다.
- 링크드 메쉬를 애니메이트 모드에서 정점 드래그로 디폼할 수 있다. 기하 편집은 셋업에서 계속 차단한다.
- 별도 수정: 뷰포트 외곽선을 선분으로 그려 miter 돌출을 없앴다.

### 2026-10-31 — M5 구현: 물리 제약

- 모델: `Document.physics`, `physics_globals`, `Animation.physics`. 필드 이름과 기본값은 4.2 형식을 따른다. 제약 이름은 종류와 무관하게 유일하고 IK와 같은 `order` 스케줄을 쓴다. 스킨 소유·이름 변경·삭제 참조를 정리한다.
- 평가: `PhysicsMode`, `PhysicsState`, `Pose::compute_physics`, `Pose::simulate`, 이어서 계산하는 `PhysicsRun`, 스켈레톤 이동 `translate`/`rotate`/`MovePath`. 프레임은 정수로 세어 결정적이다.
- op: `add_physics`, `set_physics`, `remove_physics`, `key_physics`, `key_physics_reset`, `set_physics_globals`, 그리고 문서에 없던 `remove_physics_key`(도프시트 키 삭제 동등성에 필요).
- CLI/HTTP/MCP: `pose`·`render`에 `physics`, `fps`, `move`. 애니메이션이 있으면 기본 시뮬레이션이고 셋업 포즈는 시뮬레이션하지 않는다. `pose` 응답에 제약별 오프셋·속도를 포함한다.
- 에디터: 재생 중 라이브 상태 유지, 스크럽 캐시(`physicsview.rs`), 일시정지·리셋(Shift+R)·끄기, 셋업 물리 미리보기, 아웃라이너·새 제약 대화상자(체인 일괄 생성)·인스펙터(프리셋, 전역 토글, 흔들어 보기)·도프시트 행·뷰포트 표시·팔레트·F1·한국어. `AI_GUIDE.md`와 매뉴얼을 갱신했다.

**계획과 다른 결정 (사용자 확인 필요)**
- 적분기는 자체 구현(고정 스텝 반암시적 오일러)이다(§6.1-5). 형식과 기본값은 같지만 **수치 결과는 외부 런타임과 같지 않다.**
- 이름 `""` 타임라인: 리셋은 모든 제약, 속성 키는 해당 `*_global` 플래그를 켠 제약에만 적용한다. 문서의 "모든 제약 대상"보다 좁다.
- `mass` 키는 `mass_inverse` 값을 저장한다.
- `--move`는 물리 반응만 만들고 렌더 위치는 옮기지 않는다.

§4.4 완료 정의 확인:

- [x] 모델·serde 기본값: 기본값 생략·왕복, 예제 문서 5개 검증. 마이그레이션 불필요.
- [x] `validate.rs`·잘못된 입력: 18종 잘못된 op의 배치 원자성, 잘못된 타임라인 파일 거부.
- [x] op·참조 정리: 이름 변경, 본 이름 변경·삭제, 스킨, 키 삭제, 제약 순서.
- [x] 포즈·수치: 결정성, 이어 계산 = 개별 시뮬레이션, 지연과 자식 추종, 정지 유지, 중력·바람 평형값, 속도 제한, 극단값 유한, 리셋(루프 경계 포함), 모드별 동작, 스킨 게이트, 전역. 외부 런타임 기대값은 위 사유로 제외.
- [x] CLI/HTTP/MCP·가이드: 같은 6개 op 결과 바이트 일치, 실패 롤백, Undo, 세 경로 `pose` 응답 일치(`physics`, `move` 포함).
- [ ] 에디터 UI·한국어: 연결과 번역 테이블 테스트, 셋업/애니메이트/도프시트 스크린샷만 확인. **실제 마우스·키 조작, 시간에 따른 움직임, 한국어 화면, 인스펙터 하단(전역 값·흔들어 보기·리셋 키) 실제 표시는 미검증.** 상단 컨트롤은 아이콘이 아닌 텍스트 버튼이다.
- [x] 에디터 전용 로직 없음: 폼 op의 JSON 왕복과 `apply_any` 결과 비교 테스트. 미리보기는 문서를 바꾸지 않는다.
- [x] 테스트·Clippy: 아래 결과 참고.

백업: `C:\Users\dnwls\AppData\Local\Temp\Skel2D-M5-20261031-104410` (변경 전 `crates/`·`docs/`·`examples/`). 재현 스크립트 `qa.mjs`, 결과 `qa/`. 커밋·브랜치를 만들지 않았고 M6은 시작하지 않았다.

### 2026-10-31 — 외부 소스 기반 구현 재구현

- `LocalTransform::world`의 상속 모드를 `Bone.java`를 보지 않고 동작 정의에서 다시 유도했다. `NoRotationOrReflection`은 부모를 회전과 상삼각 행렬로 나눠 대각 스트레치만 쓰고, `NoScale` 계열은 부모가 돌린 방향의 각도와 반사 부호로 만든다. 기존 기대값 테스트를 고치지 않고 통과했다.
- 차이: 부모 x축이 거의 붕괴한 경우(길이 0.001 이하 또는 방향 벡터 0)의 극단값은 이전과 다를 수 있다. 유한값 테스트로만 확인한다.
- `ik.rs`는 코사인 법칙 기반의 일반적인 해석해이고 `IkConstraint` 이식 흔적(분기 구조, 고유 상수)이 없어 바꾸지 않았다. 원본과 줄 단위 비교는 독립성을 위해 하지 않았다.
- 테스트 기대값은 호환 동작을 나타내는 데이터로 유지한다.


### 2026-10-31 — UI/UX 재개발과 root 상시 존재

- 사용자 결정: 기존 도구의 인상을 버리고 접근성·사용성·시인성·가독성 위주로 다시 만든다. 게임은 자체 런타임으로 재생하므로 외부 JSON 내보내기는 목표에서 뺀다(P9 수정, §6.1-3 대체). 설계: `docs/design/UI_REDESIGN.md`.
- root: `Document::with_root`, `ops::ensure_root`. 새 문서와 `load`에서 보장하고, 부모 없는 새 본은 root 아래로 들어간다. root는 삭제·이름 변경·부모 지정 불가. `add_bone` root는 기존 root 값을 덮어써 기존 스크립트를 유지한다. `from_str`은 원문 그대로 읽으므로 root 없는 문서도 코어에서 유효하다.
- 에디터: 뼈대·이미지·메쉬·가중치·애니메이션 5단계. 슬롯 이미지 카드와 이미지 추가, 메쉬·가중치 상태 문구, 비활성 이유 표시, 시작 화면, UI 크기 설정, 한국어·영어 혼용 제거. 눈 깜빡임·립싱크 생성기는 에디터에서 제거했고 op는 AI용으로 유지한다. 애니메이션 단계에서 이미지 카드를 눌러 키를 찍는다.
- 검증: 테스트 216개 통과, Clippy 통과, 각 단계 스크린샷 확인. 실제 마우스 조작은 미검증이다. 카드 클릭만 egui 합성 입력으로 확인했다. `docs/manual/*.png` 8장은 옛 화면이다.

### 2026-10-31 — 샘플 포팅과 메쉬 정점 이동 수정

- 외부 샘플 Celestial Circus를 `examples/celestial-circus/`로 변환했다(`convert.py`, 이미지 비상업 라이선스 `license.txt` 동봉). 물리 생략값·바람 배율은 추정값이고, transform 제약 3개는 키로 굽고, multiply 8개는 일반 블렌드다.
- `set_vertices`가 정점 위치를 바꾸면 `mesh::refit_triangles`로 삼각형을 다시 계산한다. 정점 번호를 유지하므로 가중치·디폼 키는 그대로다. `hull`이 있는 메쉬만 대상이고, 위치가 그대로인 패치는 삼각형을 건드리지 않는다. 재계산이 불가능한 배치는 중간 단계로 허용하고 최종 검증이 거절한다.
- 남은 거절: 외곽선 자체 교차, 정점이 외곽선 밖. 자체 교차도 `triangle lies outside the stored hull`로 표시되어 문구 수정이 필요하다.
- 검증: 테스트 220개 통과, Clippy 통과. 샘플 메쉬 47개에서 무작위 252회 이동 중 236회 통과. 에디터 실제 드래그는 미검증. 커밋하지 않았다.

### 2026-10-31 — 거절 문구와 물리 기본값

- 거절 문구: `crates/skel2d-editor/src/errors.rs`가 코어 메시지를 쉬운 말과 해결 방법으로 바꿔 상태바에 표시한다(영어·한국어). 코어 메시지는 AI·CLI용으로 유지한다. 드래그 중 거절된 뒤 유효한 위치로 돌아오면 문구를 지운다.
- 코어 진단: 외곽선 자체 교차를 `hull crosses itself`로 먼저 보고한다. 삼각분할 검사를 삼각형 위치 검사 앞으로 옮겼다. 곧은 변을 따라 생기는 얇은 삼각형을 생성과 검증이 다르게 판정하던 버그를 고쳤다(로프 메쉬 거절의 원인).
- 물리 기본값: 생략 시 `inertia` 0.5, `damping` 0.85(전역 값 포함). 이전 1, 1은 감쇠가 없어 계속 흔들렸다. **이 값을 생략해 저장한 기존 파일은 움직임이 달라진다.** 저장소의 예제 중 물리가 있는 파일은 값을 명시한 `celestial-circus`뿐이라 렌더가 그대로다.
- 검증: 테스트 225개 통과, Clippy 통과. 샘플 메쉬 무작위 252회 이동 중 246회 통과, 6회는 외곽선 자체 교차로 거절. 에디터에서 문구가 뜨는 모습과 실제 드래그는 미검증. 커밋하지 않았다.

### 2026-10-31 — 샘플 포팅 후속 4~8번

백업: `C:\Users\dnwls\AppData\Local\Temp\Skel2D-P4to8-20261031-161137`.

- Transform 제약(P3 일부): `Document.transforms`, op `add_transform`·`set_transform`·`remove_transform`, 평가 `transform.rs`. 월드/로컬, 절대/상대, 음수 믹스, IK·물리와 같은 평가 순서, 스킨 연동을 지원한다. 에디터 이름은 "따라하기 제약"(본 패널·명령 검색·제약 목록·속성 패널). 믹스 키는 없다. 상대 모드의 정의는 `TransformConstraint` 주석 기준의 자체 구현이다. 생략한 믹스의 기본값은 1이다.
- 블렌드: `Slot.blend`(normal·additive·multiply·screen), CPU 렌더와 에디터 모두 적용. 에디터는 슬롯 단위 wgpu 콜백(`blendpass.rs`)만 추가했고, 뷰포트 전체를 wgpu 파이프라인으로 옮긴 것은 아니다. 대상 표면이 sRGB면 일반 블렌드로 그린다. 다크 틴트는 없다.
- 본별 바인드: `Weight.offset`. 정점의 가중 평균이 0이라 셋업 위치는 그대로다. 가중치를 편집하면 `apply` 끝에서 다시 평균 0으로 맞추고, 가중치를 새로 계산하는 op(자동 가중치, 스무스 등)는 오프셋을 버린다.
- 채널 곡선: `Key.curves`, `key_bone`·`key_slot`의 `curves`. 에디터 곡선 편집기에 X/Y 선택을 추가했고 복사·이동에서 유지한다. 한 곡선을 지정하면 채널 곡선은 지워진다. 색상 채널은 op로만 지정한다.
- 복잡한 리그 표시: 이미지·메쉬 단계는 선택·호버가 아닌 본을 얇게, 가중치 단계는 해당 메쉬의 본만 또렷하게 그린다. 본이 30개를 넘으면 뼈대·애니메이션 단계도 얇게 그리고 물리 표시는 선택한 것만 보인다.
- 샘플 재변환: transform 3개를 제약으로 옮겼고(`girl/face-control`은 물리와 이름이 겹쳐 `-transform` 접미사), multiply 8개, 최대 5.12유닛 바인드를 보존한다. X/Y 곡선 구간 6곳 중 2곳만 채널 곡선이 됐다. 나머지 4곳은 같은 값으로 돌아오는 구간이라 0..1 곡선으로 표현할 수 없어 1/60초 키로 남는다.
- 검증: 테스트 238개 통과, Clippy 통과. 셋업에서 제약 3개의 이동량 0.0, `swing` 물리 중 최대 약 15유닛 추가 이동. 곱하기는 렌더와 에디터 뷰포트 모두 끈 화면과 비교해 밝아진 픽셀 0. `mix_y` 생략 시 `mix_x`를 쓰는 변환은 추정이다. 원본 재생과 직접 비교하지 않았다. 패널 조작과 실제 드래그는 미검증이다. 커밋하지 않았다.

### 2026-10-31 — 에디터 표시 수정

- 애니메이션 선택 상자: 우클릭 메뉴와 팝업 ID가 같아 왼쪽 클릭마다 목록이 바로 닫혔다. 우클릭 메뉴에 별도 ID를 줬다. 수정 전 실패하던 합성 클릭 테스트가 통과한다.
- 본 트리: 깊을수록 들여쓰기 폭을 줄이고(패널 폭의 40% 이내), 이름을 메타보다 먼저 표시하며, 잘린 이름은 마우스를 올리면 전체를 보여 준다. 가로 스크롤은 넣지 않았다.
- 흐린 본: 실선 대신 윤곽선만 있는 본 모양으로 그린다. `이름`을 켜면 흐린 본에도 이름을 표시한다.
- 깊은 트리 행과 드롭다운은 실제 화면·마우스로 확인하지 않았다. 커밋하지 않았다.

### 2026-10-31 — 팝업 재디자인, 키·op 점검, 본 색상

백업: `C:\Users\dnwls\AppData\Local\Temp\Skel2D-UI2-20261031-170046`.

- 팝업: `theme::modal`, `dialog_title`, `dialog_button`, `field`, `segmented`, `choice_card`, `error_banner`, `summary_banner`로 공통화했다. 종료 확인, 메쉬, 가중치, 제약, PSD, 파일 변경 창에 적용했다. 메쉬 고급 값은 "세부 조정"으로 접었다. 스크립트 확인용 `--dialog` 인자를 추가했다.
- 키 점검: Transform 믹스 키(`key_transform`, `remove_transform_key`, `Animation.transforms`, 도프시트 행)를 추가했다. 곡선 메뉴·편집기는 뼈대 키뿐 아니라 곡선이 있는 모든 키(슬롯 색상, 디폼, IK, Transform, 물리)에 적용한다.
- 모델에 아직 없어서 키로 만들 수 없는 것: 드로우 오더, 이벤트, IK 굽힘 방향, 물리 영향도(x·y·회전 등), 슬롯 블렌드, 상속 방식, 본 길이.
- op 점검: 에디터에 없던 op는 `clear_timeline`, `generate_blink`, `generate_lipsync` 3개였다. 도프시트 우클릭 메뉴와 명령 검색·슬롯 패널의 설명 창으로 추가했다. 색상 채널별 곡선(`key_slot.curves`)은 여전히 op 전용이다.
- 본 표시: `Bone.color`(`set_bone.color`, null이면 기본색), 속성 패널 색상 선택과 "자식에도 적용". 흐린 본은 어두운 테두리 위에 채운 모양으로 그려 밝은 이미지에서도 구분된다. 샘플은 원본의 본 색상을 가져온다.
- 검증: 테스트와 Clippy 통과, 팝업 5종과 본 표시 스크린샷 확인. 버튼을 실제로 눌러 적용하는 조작과 믹스 키 버튼은 마우스로 확인하지 않았다. 커밋하지 않았다.

### 2026-10-31 — 눈 깜빡임·립싱크 생성기 제거

- 사용자 결정: 참조한 편집기들에 없고 꼭 필요하지 않으므로 전부 제거한다. `generate_blink`, `generate_lipsync`, `MouthShapes`, `lipsync.rs`, 에디터 창과 메뉴, 문서를 지웠다. 이 op를 쓰던 외부 스크립트는 이제 파싱 오류가 난다. `examples/face/face.ops.json`은 생성 결과를 `key_slot`으로 풀어 썼다.
- 유지: 슬롯 이미지를 시간별로 바꾸는 `key_slot`, 아래 트랙으로 되돌리는 `reset`, 이미 저장된 애니메이션. 깜빡임과 입 모양은 5단계에서 이미지 카드를 눌러 만든다.

### 2026-10-31 — Spineboy 포팅

- `examples/spineboy/`에 spineboy-pro를 변환했다(이미지 비상업 라이선스 `license.txt` 동봉). 변환기를 `examples/convert_spine.py` 하나로 합쳤고, Celestial Circus 재변환 결과는 이전 파일과 바이트 단위로 같다.
- 새로 변환하는 것: IK, IK·Transform 믹스 키, 슬롯 색상 키, 셰어 키, 디폼 키, 본 스케일·상속, 슬롯 setup 색상.
- 표현하지 못해 버린 것: 클리핑 어태치먼트, 바운딩 박스, 이벤트(`footstep`), IK 소프트니스 키(`walk`, `run`, `portal` 등의 다리). IK 굽힘 키는 모두 setup과 같은 값이라 손실이 없다. 렌더 7종은 눈으로 확인했고 원본 재생과 직접 비교하지 않았다. 커밋하지 않았다.

### 2026-10-31 — 드로우 오더 키, 이벤트, IK 확장

백업: `C:\Users\dnwls\AppData\Local\Temp\Skel2D-AnimKeys-20261031-173556`.

- 드로우 오더: `Animation.draw_order`, `key_draw_order`, `remove_draw_order_key`, `Pose.draw_order`. 오프셋 대신 슬롯 이름 전체 목록을 저장한다. `order`를 생략하면 셋업 순서다. 슬롯 이름 변경·삭제를 따라가고, 키 이후 추가된 슬롯은 셋업상 아래 슬롯 바로 위에 놓인다. 여러 트랙이면 alpha 0.5 이상인 마지막 트랙의 키가 이긴다. 렌더와 에디터 표시·선택이 이 순서를 쓴다.
- 이벤트: `Document.events`, `Animation.events`, `add_event`, `set_event`, `remove_event`, `key_event`, `remove_event_key`, `pose::fired_events`. 키에서 생략한 값은 정의의 기본값이다. `pose`는 `events`를 보고하고 `--since`(HTTP·MCP `since`)로 구간을 정한다. 렌더 이미지에는 그리지 않는다. 오디오·볼륨·밸런스는 범위 밖이라 변환기가 보고만 한다.
- IK: `softness`, `compress`, `stretch`와 키 `IkKey`. 키의 생략 값은 셋업 값을 따른다. 혼합값만 있는 키는 숫자 그대로 저장해 기존 파일과 호환한다. `mix`, `softness`는 곡선 채널 0, 1로 보간하고 굽힘·압축·늘이기는 다음 키까지 유지한다. `key_ik`의 `mix`는 이제 선택 항목이고, 같은 시간의 키에 합쳐진다.
- 소프트니스는 동작 정의에서 직접 유도했다. 도달 거리 ± softness 구간을 포물선으로 이어 양 끝에 꺾임이 없다. 단위는 월드 거리다. 외부 런타임과 수치를 직접 비교하지 않았다.
- 압축은 한 본 체인에만 적용한다(두 본은 값이 있어도 효과 없음, 에디터에서 비활성). `uniform`은 요청 범위 밖이라 없고 변환기가 보고한다. 늘이기는 x 스케일만 바꾸므로 부모 스케일이 자식을 기울일 수 있으며, 자식 조준을 반복 보정해 끝점을 맞춘다.
- 에디터: 도프시트 그리기 순서·이벤트 행, IK 인스펙터 값별 키 버튼, 5단계 그리기 순서·이벤트 그룹, 재생 중 이벤트 알림, 무릎 핸들과 `I`의 키 기록, 명령 검색, 한국어. 이벤트 기본값은 5단계에서도 "기본값" 제목 아래에서 정의를 직접 고친다. IK 채널별 곡선은 op 전용이고, 에디터에서 곡선을 고치면 한 곡선으로 합쳐진다.
- 샘플: `py -3 examples/convert_spine.py C:\Users\dnwls\Downloads\spineboy-4.3 spineboy-pro examples\spineboy spineboy`. 이벤트 키 5개, IK 키 29개. 남은 미표현은 클리핑과 바운딩 박스 어태치먼트다. Celestial Circus 재변환은 바이트 단위로 같다. 변환기의 드로우 오더 변환은 샘플이 없어 별도 스크립트로만 확인했다.
- 검증: 테스트 261개 통과, `cargo clippy --workspace --release -- -D warnings` 통과, 예제 7개 validate 통과, walk·run 렌더와 에디터 스크린샷 확인. walk 0.25초 앞 정강이 약 1.5유닛, run 0.3초 뒤 정강이 약 3.9유닛 이동했다. `--all-targets`를 붙인 Clippy는 에디터 테스트 코드에서 실패하며(`theme.rs`, `physics.rs`, `stepui.rs` 등) 이번에 고치지 않았다.
- 미검증: 실제 마우스 조작 전부. 무릎 핸들, `I`, 패널 버튼·체크박스, 도프시트 키 드래그·박스 선택·삭제·붙여넣기, 이벤트 대화상자, 명령 검색, 재생 중 이벤트 알림, 키로 바뀐 순서에서의 뷰포트 선택. op 왕복 단위 테스트와 스크린샷으로만 확인했다. 원본 재생과 비교하지 않았다. 커밋하지 않았다.

### 2026-10-31 — 본 색상, 뷰포트 표식, 열기 전 저장 확인, 프로그램 아이콘

- 본 색상: 트리 아이콘·이름, 속성 패널, 도프시트 라벨·키가 `Bone.color`를 따른다. 글자는 `theme::legible`로 어두운 색만 밝혀 대비 4.5:1을 맞춘다.
- 뷰포트 표식: IK 타깃은 제약 아이콘, 길이 0인 본은 십자 표적, 물리 본은 물리 아이콘. 길이 있는 본과 선택 범위는 그대로다. 슬롯·이미지는 뷰포트에 원래 표식이 없어 추가하지 않았고, Transform 제약도 표식 없음.
- 새로 만들기·열기·예제 열기: 수정 사항이 있으면 저장 / 저장 안 함 / 취소 대화상자를 먼저 띄운다. 저장 실패 시 진행하지 않는다.
- 프로그램 아이콘: 창 아이콘은 이미 있었고, `build.rs`와 `winresource`로 기존 `assets/app_icon.png`에서 만든 `app_icon.ico`를 exe에 넣었다. 새 빌드 의존성이며 Windows SDK의 `rc.exe`를 쓴다.
- 미검증: 메뉴·Ctrl+O·대화상자 버튼 실제 클릭과 파일 선택 창, 표식 호버·선택, 물리 아이콘 화면 표시(Spineboy에 물리 없음). 커밋하지 않았다.

### 2026-10-31 — 키 작업 개선, 슬롯 표시, 제약 배지, 토스트, 메쉬 드래그, 빌드 시간

백업: `C:\Users\dnwls\AppData\Local\Temp\Skel2D-KeyFlow-20261031-203508`.

- 참고: 외부 문서의 keys, dopesheet, meshes 항목(코드 미참조).
- 키: K는 변경한 값만 기록하고, 변경이 없으면 현재 툴 속성, 슬롯 이미지, 선택한 IK 순으로 기록한다. Ctrl+Shift+L은 보이는 행 전체를 기록한다. 자동 키를 끄면 본·슬롯·디폼·IK·Transform·물리 값 편집이 모두 미키 상태로 보류된다. 재생 헤드를 옮기면 버리고 개수를 알린다. 드로우 오더와 이벤트는 바로 기록한다.
- 도프시트: 요약 행, 본·슬롯의 속성별 행, 보간선, 행 클릭 선택, 더블클릭 삭제, Ctrl+A, Ctrl+Shift+드래그 복제, 선택 본만 표시와 "모든 행" 토글. 속성 종류 필터는 하지 않았다.
- 트리: 1·5단계에 슬롯과 이미지 표시("슬롯" 토글), 키 버튼, 제약 배지, 인스펙터 경계 드래그. 토스트: `toasts.rs`. `status` 문구를 프레임마다 토스트로 옮기고 종류는 키워드로 판단한다.
- 메쉬 드래그 막힘: 본 이동이 모든 메쉬를 재삼각분할 검사해 정점 하나가 외곽선을 벗어나면 드래그 전체를 거절했다. 이제 `validate_topology`는 구조만 검사하고 위치 문제는 `mesh::geometry_problem`으로 알린다. 꼬인 메쉬는 마지막 삼각형을 유지하고 저장도 가능하며, 삼각형을 다시 만드는 op만 거절한다. **코어 검증 완화**: 꼬인 메쉬가 CLI·HTTP·MCP에서도 유효하다.
- 빌드: `release`는 증분 빌드로, 기존 LTO 설정은 `dist` 프로필로 옮겼다. 배포 빌드는 `cargo build --profile dist`.
- 미검증: 실제 마우스 조작 전부(도프시트 드래그·복제·박스 선택, 정점 Ctrl 선택, 트리 키 버튼, 배지와 +N 메뉴, 경계 드래그, 토스트 호버·닫기). 커밋하지 않았다.
