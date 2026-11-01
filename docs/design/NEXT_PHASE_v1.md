> **2026-10-31 정정:** 이 문서의 외부 런타임 소스 이식·"그대로 옮긴다" 지시는 폐기한다. 타사 코드를 이식하지 않고 동작과 형식을 기준으로 직접 구현한다(`NEXT_PHASE.md` §6.1-5).

# Skel2D 다음 단계 설계 문서 (Codex 인계용)

작성: 2026-10-30 · 대상: 이 저장소를 이어서 구현할 에이전트(Codex)
범위: 널리 쓰이는 2D 스켈레탈 런타임 4.2 세대의 기능 동등성 확보와 그 과정의 UI/UX 개선. 현재 코드 상태(2026-10-29 기준)를 출발점으로 한다.

---

## 0. 먼저 읽을 것 (작업 규칙)

### 0.1 제품 원칙 (우선순위 순)
1. **AI가 모든 것을 제어할 수 있어야 한다.** 에디터에서 할 수 있는 일은 전부 op(`skel2d_core::Op`)로도 할 수 있어야 하고, CLI·HTTP(`serve`)·MCP에서 같은 op로 호출된다. 에디터 전용 상태가 유일한 수단이 되면 안 된다.
2. **에디터는 익숙한 작업 환경**을 준다(트리, 셋업/애니메이트 모드, 툴, 키 버튼 색, 단축키).
3. **작업 중 불편하기 쉬운 점은 고친다.** 고친 지점은 코드 주석과 이 문서에 남긴다.

### 0.2 불변 규칙
- 모든 문서 요소는 **이름으로 주소 지정**한다(인덱스 금지). 이름 변경·삭제 시 모든 참조(애니메이션, 스킨, 제약, 가중치)를 같이 갱신한다. 기존 패턴: `ops.rs`의 `rename_bone`, `remove_bone`, `set_slot`.
- op 배치는 **원자적**이다(`skel2d_core::apply` → 실패 시 문서 불변, 성공 시 `validate` 통과).
- 파일 포맷은 `skel2d/1` JSON. 새 필드는 `#[serde(default, skip_serializing_if = ...)]`로 추가해 **기존 파일이 그대로 열리게** 한다. 구조를 바꿔야 하면 `skel2d_core::from_str`의 마이그레이션 단계에 넣는다(예: `lib.rs::migrate_expressions`).
- 새 모델 필드/타입마다: `validate.rs` 검사 + op + 테스트 + `docs/AI_GUIDE.md` 한 줄.
- UI 문자열은 영어로 쓰고 `crate::i18n::tr("...")` / `crate::tf!("... {}", x)`로 감싼다. 한국어는 `crates/skel2d-editor/src/i18n/ko_*.rs`에 추가. 같은 영어 키는 모든 표에서 같은 번역이어야 한다(테스트가 검사). 다른 의미면 다른 영어 키를 쓴다(예: 이미지 치수는 `"Dimensions"`, 브러시는 `"Size"`).
- 색·크기는 `theme.rs` 상수와 헬퍼만 쓴다(리터럴 금지).
- 아이콘: `crates/skel2d-editor/assets/icons/*.png` 64×64 RGBA, 흰색 글리프 + 투명 배경. 새 아이콘은 `icons.rs`의 `Icon` enum과 `PNGS` 배열에 **같은 순서로** 추가.

### 0.3 작업 위생 (실제로 사고가 났던 것들)
- **저장소에 커밋이 하나도 없고, 사용자가 커밋을 보류했다(§6-4).** 커밋하지 말고, 큰 변경 전마다 `crates/`·`docs/`·`examples/`를 임시 폴더로 백업한다.
- `perl -pi -e 's|...|...|'`처럼 **`|`를 구분자로 쓰면서 패턴 안에 `\|`(Rust 클로저 `|x|`)가 들어가면** 치환이 깨져 파일 첫 줄의 글자가 지워지거나 엉뚱한 곳에 코드가 삽입된다. 여러 줄 치환은 편집 도구(정확한 문자열 치환)로 하고, sed는 `#` 구분자 + 한 줄 치환에만 쓴다.
- 매 단계 끝에: `cargo test --workspace --release`, `cargo clippy --workspace --release` 경고 0, 에디터 스크린샷 확인:
  `target/release/skel2d-editor FILE [--anim A --time T --select BONE --select-slot SLOT --tool NAME --skin K --tracks a,b] --size 1280x800 --screenshot out.png [--wait 1]`
  (렌더 결과는 `skel2d render`로 확인)

---

## 1. 현재 구조 요약

### 1.1 크레이트
| 크레이트 | 역할 | 핵심 파일 |
|---|---|---|
| `skel2d-core` | 모델, op, 포즈 계산, 검증, 포맷 | `model.rs`, `ops.rs`, `pose.rs`, `ik.rs`, `curve.rs`, `validate.rs`, `lib.rs`(로드·마이그레이션) |
| `skel2d-render` | tiny-skia CPU 렌더러(PNG) | `lib.rs` |
| `skel2d-rig` | 자동 리깅(이미지 분석, 메쉬, 지오데식 가중치) | `mesh.rs`, `weights.rs`, `layers.rs` |
| `skel2d-cli` | `skel2d` CLI, HTTP, MCP | `main.rs`(인자), `commands.rs`(`render_bytes`), `serve.rs`, `mcp.rs`, `session.rs` |
| `skel2d-editor` | egui/eframe 0.37 에디터(wgpu) | `app.rs`, `panels.rs`, `viewport.rs`, `timeline.rs`, `meshedit.rs`, `import.rs`, `theme.rs`, `icons.rs`, `palette.rs`, `i18n/` |

### 1.2 포즈 파이프라인 (`pose.rs::Pose::compute_skinned(doc, tracks, skins)`)
1. 셋업 로컬 변환 → 트랙 순서대로 본 타임라인(rotate/translate/scale, 셋업 기준 오프셋·배율)을 alpha로 섞음.
2. 월드 변환 → IK(`doc.ik` 순서, 스킨 소유·비활성 본 제외) → 월드 재계산.
3. 스키닝 행렬 `skin = world * setup_world⁻¹`.
4. 슬롯: 색(셋업 색 × 키 색), 어태치먼트(키 `reset`은 아래 트랙 값 유지), deform 오프셋.
5. `Pose { bones, slots, skins, active }`. 어태치먼트는 반드시 `pose.attachment(doc, slot_index)`로 해석(스킨 반영).

**현재 포즈는 시간의 순수 함수**다. 물리(§3.5)가 이 가정을 깬다.

### 1.3 에디터 레이아웃과 재사용 부품
- 레이아웃(`app.rs::ui`): 상단 메뉴, 하단 상태바, 왼쪽 툴 레일, 오른쪽 사이드(위: 아웃라이너 `panels::tree`, 아래: 인스펙터 `panels::properties`), 애니메이트 모드에서 하단 타임라인, 중앙 뷰포트. 메쉬/가중치 툴 활성 시 뷰포트 위 떠 있는 창(`panels::mesh_tool_window`).
- 아웃라이너: `Tab` enum(계층/순서/제약/애니/스킨), 행은 전부 `tree_row(ui, tc, RowSpec{..}) -> RowOut`(눈·잠금 열, 접기, 들여쓰기 가이드, 보조 정보). 새 목록은 이걸로 만든다.
- 인스펙터: `inspector_header`(타입 아이콘 카드 + 이름 편집), `theme::group`(접는 그룹, 오른쪽 요약), `theme::prop`(라벨 열 + 값), `num_row`, `pair_row`(X/Y 축 태그), `key_button`(키 색: 빨강=여기 키, 주황=변경 미키, 초록=보간).
- 편집은 전부 `app.edit(ops)` / `app.edit_coalesced(key, ops)` / 드래그 중 `app.preview(base, ops)`. `app.retarget_skins`가 미리보기 중인 스킨의 어태치먼트 편집을 해당 스킨으로 자동 전환한다.
- 모달: `app.rs`의 `Dialog` enum(`ExternalChange`, `ConfirmClose`, `PsdSplit`)과 `dialogs()`에서 `egui::Modal` 사용.

### 1.4 감사로 확인된 미완 목록
- 애니메이션: 커브 편집이 본 키만 가능, IK bend 키 불가, 전체 본 키 없음, 키 없는 슬롯/IK는 도프시트 행이 안 보임, 애니메이션 복제 없음, X/Y 분리 키 불가, 그래프 에디터 없음, 드로우 오더 타임라인·이벤트 없음, 레이어 순서가 `BTreeSet`(이름순)이라 트랙 순서 지정 불가.
- 리깅: 셰어, 상속 모드, IK softness/stretch/compress/uniform, 트랜스폼·패스·물리 제약, 제약 순서, 블렌드 모드, 다크 틴트 없음.
- 어태치먼트: 바운딩 박스, 클리핑, 패스, 포인트, 링크드 메쉬, 시퀀스 없음.
- 메쉬: 수동 엣지, 트레이스 파라미터, UV 리셋 없음.
- UI: 새 애니메이션/이름 변경/삭제 확인/환경설정 등 팝업 대화상자 부족.
- 내보내기: 외부 JSON, 아틀라스, GIF/비디오 없음.

---

## 2. 작업 순서

의존성 때문에 이 순서를 지킨다. 각 단계는 독립적으로 빌드·테스트 가능한 상태로 끝낸다.

| 단계 | 내용 | 이유 |
|---|---|---|
| **P0** | 공통 기반: 제약 순서 모델, 대화상자 시스템, 트랙 순서(Vec) | 이후 모든 제약·팝업이 이 위에 올라감 |
| **P1** | 본 셰어 + 상속 모드 | 변환 수학 변경, 모든 제약의 전제 |
| **P2** | IK 확장(softness/stretch/compress/uniform, bend 키) | 기존 IK 개선, 타임라인 일반화의 첫 사례 |
| **P3** | 트랜스폼 제약 | P1 변환 위에서 단순 |
| **P4** | 물리 제약 | 포즈 계산을 상태 있는 시뮬레이션으로 확장 |
| **P5** | 블렌드 모드 + 다크 틴트 (렌더러·뷰포트 셰이더) | 뷰포트 렌더링 구조 변경 |
| **P6** | 애니메이션 보강(커브 일반화, 키 전체, X/Y 분리, 도프시트, 복제, 드로우 오더·이벤트 타임라인, 그래프 에디터) | |
| **P7** | 어태치먼트 확장(포인트, 바운딩 박스, 클리핑, 링크드 메쉬, 패스 + 패스 제약) | |
| **P8** | 메쉬 도구 보강(수동 엣지, 트레이스 옵션, UV 리셋) | |
| **P9** | 내보내기(외부 JSON 4.2, 아틀라스, PNG 시퀀스/GIF/비디오) | 모델이 안정된 뒤 |

---

## 3. 단계별 설계

### 진행 기록 (2026-10-30, Codex)
- P0-1 구현: IK `order`(기본 0), 안정 정렬 기반 `ConstraintRef` 평가, 영향받는 하위 트리 월드 재계산, `set_ik.order`, 원자적 `set_constraint_order { names }`.
- 순서 op는 **모든 제약 이름을 정확히 한 번씩** 받는다. 빈 문서는 빈 목록을 허용한다. 음수·같은 order도 유효하며, 같은 값은 기존 배열 순서를 유지한다.
- 에디터 제약 탭: 적용 순서대로 표시, 순번, 드래그 정렬, 인스펙터 순서 편집, Ctrl+K의 먼저/나중 적용 명령과 한국어 번역. `--select-ik NAME`으로 제약 탭·인스펙터를 지정해 캡처 가능.
- 현재 모델에 있는 IK만 평가 목록에 등록했다. 트랜스폼·물리·패스는 해당 단계에서 같은 스케줄에 추가한다.
- 검증: `cargo test --workspace --release` 88개 통과, `cargo clippy --workspace --release -- -D warnings` 경고 0, 릴리스 빌드 성공. CLI 순서 변경·렌더 성공. 셋업/애니메이트 화면과 렌더 PNG를 열어 확인했다 (`target/qa/p0-1/{setup,animate,render}.png`). 드래그 제스처 자체의 수동 검증은 하지 않았다.
- P0-2 구현: `dialogs.rs`의 공통 `DialogResult` 모달과 7개 폼(새/복제 애니메이션, 새 스킨, 새 IK, 이름 변경, 삭제 확인, 메쉬 생성 옵션, 환경설정). 메뉴·팔레트·Tab·F2·Del·Ctrl+, 연결. 빈 이름/중복 안내, 첫 이름 입력 포커스, Enter/Esc, 실패 시 폼 유지. 기존 PSD/외부 변경/종료 모달에도 Enter/Esc 적용.
- 이름 변경은 본·슬롯·IK·스킨·애니메이션을 지원하며 참조 수를 표시하고 선택·트랙·스킨 미리보기 이름도 갱신한다. 삭제 영향은 실제 core op 사전 적용으로 집계하고, 단순 항목은 즉시 삭제 후 Undo 안내. 아웃라이너 Up/Down은 검색·접힘을 반영한 표시 행을 순회한다.
- `duplicate_animation {name,new_name}` op를 P6에서 앞당겨 추가했다. 새 애니메이션 폼의 FPS는 기존 타임라인 환경설정 FPS를 갱신한다(애니메이션 파일 필드 추가 없음). 스킨 복사는 미리보기 순서대로 합치며 같은 어태치먼트는 나중 스킨이 우선한다.
- 메쉬 생성 폼은 현재 지원하는 밀도·균일/직사각형 격자·열/행·자동 가중치를 제공한다. **알파 임계값/패딩은 P8**, **추가 제약 종류는 P3/P4/P7**, **내보내기 폼은 P9**에서 기능 구현과 함께 연결한다. 아직 없는 기능의 버튼은 만들지 않았다. 어태치먼트 행의 이름 변경은 별도 rename op가 없어 이번 범위에 포함하지 않았다.
- P0-3 구현: `Playback.layers`를 `Vec<LayerTrack { anim, alpha, enabled }>`로 변경. 입력 순서를 유지하고, 타임라인에서 드래그/버튼 정렬·불투명도·활성·제거·추가와 팔레트 명령을 제공한다. 목록 아래부터 적용하고 현재 편집 애니메이션은 고정 트랙 0이다. 동일 애니메이션의 추가 레이어는 중복 적용 없이 설정만 보관한다.
- 일반 포즈·어니언 스킨·CLI/HTTP/MCP 렌더·포즈 조회가 공유 스케줄을 사용한다. `--layers JSON`/API `layers` 배열과 `schema tracks`를 추가했다(기존 `schema layers`는 리깅 이미지 명세). 레이어는 문서에 저장하지 않는 런타임 설정이다. 기존 파일 포맷 변경 없음.
- P0-3 검증: 릴리스 테스트 100개 통과, Clippy 경고 0, 빌드 성공. 셋업/애니메이트 캡처 확인, 실제 UI의 위/아래 버튼·드래그 정렬·활성 토글에 따른 포즈 변경 확인. CLI·HTTP 포즈 50도 일치, PNG 바이트 일치, 잘못된 alpha HTTP 400 확인. MCP 수치/렌더 테스트도 통과. 증거: `target/qa/p0-3/{setup,animate,render}.png`, `verification.json`.
- P0-3 변경 전 백업: `%TEMP%/Skel2D-P0-3-20261030-125207`. 커밋하지 않음.
- P0의 현재 기능 기반은 구현했으며, P0-2의 후속 기능 연계 항목은 위에 적은 해당 단계에서 진행한다.
- P1 구현: 본의 `shear_x/shear_y`와 다섯 `inherit` 모드, 셰어 타임라인·키·삭제/클리어·기간·복사/이동·보간·레이어링을 연결했다. 기존 파일은 기본값 생략으로 호환되며, 셰어가 없는 normal 변환은 기존 TRS 계산 경로를 유지한다. 평가 결과의 local 셰어·상속 모드·전체 world 행렬을 CLI/HTTP/MCP pose에 제공한다.
- 셋업 셰어 X/Y, 애니메이션 셰어 X/Y와 키 버튼, 상속 설명/드롭다운, 팔레트·도움말·한국어 번역을 추가했다. 상속 모드 키는 계획대로 P6이며, 애니메이트 모드에서 셋업 상속 변경은 비활성화한다. Scale 툴에서 Alt+드래그로 본 셰어(X 핸들=X축, Y/자유 핸들=Y축). 자동 키 끄기 시 미키 보관 경로도 포함한다.
- 셰어가 생기는 재부모화를 위해 world→local 분해를 추가했으며, 다섯 상속 모드에서 전체 월드 행렬을 보존한다. 역변환 불가능한 0 스케일 부모는 오류를 표시한다. 기존 IK의 월드 재계산·반사 방향도 상속 모드를 반영하며, IK 해법 자체의 확장은 예정대로 P2에서 진행한다.
- P1 검증: 릴리스 테스트 **111개 통과**, Clippy `-D warnings` 경고 0, 릴리스 빌드 성공. 부모 회전·비균일 스케일·셰어·반사의 기준 수치, 0 스케일, 스키닝, 재부모화, 키 참조/이동/복사, Alt+Scale 계산 테스트와 MCP op→pose/render 테스트 통과. CLI/HTTP 포즈·PNG 일치. 셋업/애니메이트/렌더 캡처(`target/qa/p1/{setup,animate,render}.png`)와 실제 인스펙터의 셰어 키/상속 안내·모드별 활성 상태를 확인했다. Alt 키를 누른 물리적 드래그 제스처는 단위 테스트로 검증했으며, UI 자동화에서는 재현하지 않았다.
- P1 변경 전 백업: `%TEMP%/Skel2D-P1-20261030-164403`. 커밋하지 않음.
- 다음 작업: **P2 IK 확장**.
- P0-2 변경 전 백업: `%TEMP%/Skel2D-P0-2-20261030-122025`. 커밋하지 않음.
- P0-2 검증: `cargo test --workspace --release` 93개 통과, `cargo clippy --workspace --release -- -D warnings` 경고 0, 릴리스 빌드 성공. 실제 에디터 셋업/애니메이트 화면에서 Ctrl+,·Esc, 이름 변경의 첫 포커스/참조 표시/빈 이름 차단/Enter 적용/Undo, 새 애니메이션 생성/Undo를 확인했다. 한국어 번역 생성 과정의 인코딩 오류를 수정하고 다시 화면으로 확인했다. CLI `duplicate_animation` 적용과 복제 애니메이션 렌더 성공 (`target/qa/p0-2/duplicate-render.png`).
- 변경 전 백업: `%TEMP%/Skel2D-P0-1-20261030-120946` (`crates/`, `docs/`, `examples/`). 커밋하지 않음.

각 항목은 **모델 → 평가(pose) → op → 검증 → CLI/MCP → 에디터 UI → 테스트** 순서로 쓴다.

### P0. 공통 기반

#### P0-1 제약 순서
IK·트랜스폼·패스·물리 제약을 하나의 순서로 적용한다.
- 모델: 각 제약 구조체에 `order: i32`(기본 0) 추가. 적용 순서 = `order` 오름차순, 같으면 종류별 배열 순서. (외부 JSON의 `order`와 1:1 매핑되어 내보내기가 쉬움)
- 평가: `pose.rs`에 `enum ConstraintRef { Ik(usize), Transform(usize), Path(usize), Physics(usize) }` 목록을 만들어 정렬 후 순회. 각 제약 적용 뒤 영향받는 본의 하위 트리만 월드를 다시 계산(`world_transforms`를 부분 재계산 가능하게 리팩터링).
- op: 각 `Set*` 패치에 `order: Option<i32>`. 별도로 `{"op":"set_constraint_order","names":[...]}`(나열 순서대로 0,1,2… 재배정)도 둔다.
- UI: 아웃라이너 "제약" 탭을 모든 종류를 한 목록으로 보여주고 **드래그로 순서 변경**(드로우 오더 탭의 `DropTarget::Slot` 패턴 재사용). 행 왼쪽에 순번, 아이콘은 종류별.

#### P0-2 대화상자(팝업) 시스템
사용자 요청: 인라인/우클릭 메뉴만 쓰지 말고 팝업을 쓸 것.
- 구조: `Dialog` enum을 확장하고 `dialogs()`에서 `egui::Modal`로 그린다. 각 대화상자는 `fn ui(&mut self, app, ui) -> DialogResult { Keep, Close, Apply(Vec<Op>) }` 형태의 작은 구조체로 분리해 `dialogs.rs` 모듈에 둔다(app.rs 비대화 방지).
- 공통 규칙: Enter = 확인(기본 버튼 강조), Esc = 취소, 첫 입력란 자동 포커스, 이름 중복/빈 이름은 확인 버튼 비활성 + 빨간 안내문, 파괴적 작업은 빨간 버튼(`theme::danger_button`).
- 추가할 대화상자:

| 대화상자 | 열리는 곳 | 내용 |
|---|---|---|
| 새 애니메이션 | 애니 탭 추가, Tab 키(애니 없을 때), 팔레트 | 이름, 길이(초), fps, "현재 애니메이션 복제" 체크 |
| 이름 바꾸기 | F2, 모든 행 우클릭 | 이름 입력 + 참조 수 미리보기("키 12개, 스킨 2개에서 같이 바뀜") |
| 삭제 확인 | Del, 우클릭 삭제 (자식·키가 있을 때만) | "본 3개, 슬롯 2개, 키 40개가 삭제됨" 목록. 단순 항목은 확인 없이 삭제(Undo 있음) |
| 새 스킨 | 스킨 탭 추가 | 이름, "현재 미리보기 스킨에서 복사" 체크 |
| 새 제약 | 제약 탭 추가 | 종류(IK/트랜스폼/물리/패스) 라디오 → 종류별 필수 필드(본, 타깃) 콤보, 선택된 본으로 미리 채움 |
| 메쉬 생성 옵션 | 리전 인스펙터 "메쉬로" | 밀도, 알파 임계값, 패딩, 균일 격자/열·행, 생성 후 자동 가중치 체크 |
| 내보내기 | 파일 메뉴 | 형식(외부 JSON/PNG 시퀀스/GIF/비디오), 대상 애니·스킨, 크기, fps, 출력 경로 |
| 환경설정 | 편집 메뉴, Ctrl+, | 탭: 일반(언어, UI 크기), 뷰포트(그리드, 본 이름, 어니언 프레임 수), 스냅(각도, 격자), 애니메이션(fps, 자동 키), 메쉬(본 이동 시 메쉬 따라감). 지금 흩어진 `Settings` 필드를 전부 한곳에 |

- ASCII 레이아웃 예(새 제약):
```
┌ 새 제약 ───────────────────────────┐
│ 종류   (•) IK  ( ) 트랜스폼  ( ) 물리 │
│ 이름   [ik1                      ] │
│ 본     [forearm_l ▾] [+부모 포함 ☑] │
│ 타깃   [새 타깃 본 만들기 ▾]          │
│ ⓘ 선택한 본 forearm_l로 채웠습니다.   │
│                    [취소] [만들기 ⏎] │
└────────────────────────────────────┘
```

#### P0-3 트랙(레이어) 순서
- `Playback.layers: BTreeSet<String>` → `Vec<LayerTrack { anim, alpha, enabled }>`. 순서 = 트랙 번호(아래가 먼저). 표정 포즈(0초 키 애니메이션)를 아래에, 깜빡임을 위에 두는 방식이 이 순서에 의존한다.
- UI: 타임라인 상단 "레이어" 메뉴를 작은 리스트(드래그 정렬, alpha 슬라이더, 켜기/끄기)로 교체.

---

### P1. 본 셰어 + 상속 모드

**모델** (`model.rs::Bone`)
- `shear_x: f32`, `shear_y: f32` (도, 기본 0).
- `inherit: Inherit` enum: `normal`(기본) | `only_translation` | `no_rotation_or_reflection` | `no_scale` | `no_scale_or_reflection` (4.2 형식 이름과 동일, serde snake_case).
- `BoneTimelines`에 `shear: Vec<Key<Vec2>>` (셋업 기준 오프셋).
- 상속 모드 키(4.2 형식의 `inherit` 타임라인)는 P6에서 계단식 타임라인으로 추가.

**평가**
- `LocalTransform`에 `shear_x, shear_y` 추가, `Affine::from_trs` → `from_trss(x, y, rot, sx, sy, shx, shy)`로 다음 공식 사용:
  `a = cos(rot+shx)*sx, c = cos(rot+90+shy)*sy, b = sin(rot+shx)*sx, d = sin(rot+90+shy)*sy` (`Bone.updateWorldTransform`과 동일한 정의를 따를 것).
- `world_transforms`를 `inherit` 분기로 확장. `updateWorldTransformWith`의 다섯 분기(4.2 세대 `Bone` 정의)를 그대로 옮긴다. 출처를 주석에 링크.
- 스키닝(`skin = world * setup_world⁻¹`)은 그대로 동작.

**op**: `BonePatch`에 `shear_x, shear_y, inherit`. `KeyBone`에 `shear: Option<Vec2>`.

**에디터 UI**
- 본 인스펙터 "변환" 그룹: 위치(X/Y) · 회전 · 크기(X/Y) · **셰어(X/Y)** · 길이. 상속은 별도 그룹 "상속"에 드롭다운 1개 + 한 줄 설명(선택 항목별 툴팁).
- 툴 레일에 셰어 툴 추가는 하지 않는다(인스펙터·드래그 위주). 대신 Scale 툴에서 **Alt+드래그 = 셰어**(단축키 도움말에 표기).
- 애니메이트 블록에 셰어 행 + 키 버튼.

**테스트**: 각 상속 모드별 부모 회전/스케일 시 자식 월드 값(4.2 런타임 결과를 기대값으로), 셰어 왕복 직렬화, 기존 파일 로드 불변.

---

### P2. IK 확장

**모델** (`IkConstraint`)
- 추가: `softness: f32`(0), `compress: bool`, `stretch: bool`, `uniform: bool`, `order: i32`.
- 타임라인: `Animation.ik: BTreeMap<String, Vec<Key<f32>>>`(mix만) → `Vec<Key<IkFrame>>`, `IkFrame { mix, softness, bend_positive, compress, stretch }`.
  - **마이그레이션**: `IkFrame`에 커스텀 Deserialize를 달아 숫자 하나(`0.5`)도 `{mix:0.5, 나머지는 제약의 셋업값}`으로 읽게 한다. 셋업값을 알아야 하므로 로드 후 `fill_ik_defaults(doc)` 패스에서 `Option` 필드를 채운다(직렬화 시 셋업과 같은 값은 생략해 diff를 작게 유지).
  - bool 값(bend/compress/stretch)은 보간하지 않고 계단식.

**평가** (`ik.rs::solve`): 4.2 세대 `IkConstraint.apply`의 1본/2본 알고리즘(softness, stretch, compress, uniform 포함)을 그대로 이식. 현재 해석적 2본 해법을 대체.

**op**: `IkPatch`에 새 필드, `KeyIk`에 `softness/bend_positive/compress/stretch` 선택 필드.

**에디터 UI**
- IK 인스펙터: 믹스 슬라이더, 소프트니스(px), 굽힘 방향(현재 버튼 유지), 체크 3개(늘이기/압축/균일). 애니메이트 모드에서는 각 행 앞에 `key_button`(본 애니메이트 블록과 같은 모양). **애니메이트 모드에서 굽힘 버튼·무릎 핸들 드래그는 키를 쓴다**(현재는 셋업을 몰래 바꿈 — 감사에서 지적).
- 도프시트 IK 행은 키가 없어도 표시(P6과 함께).

---

### P3. 트랜스폼 제약

**모델**: `Document.transform: Vec<TransformConstraint>`
```
name, order, bones: Vec<String>, target: String,
mix_rotate, mix_x, mix_y, mix_scale_x, mix_scale_y, mix_shear_y,   // 0..1
offset_rotation, offset_x, offset_y, offset_scale_x, offset_scale_y, offset_shear_y,
local: bool, relative: bool
```
4.2 세대 `TransformConstraintData`와 필드명을 맞춘다(내보내기 1:1).

**평가**: `TransformConstraint.apply`의 4가지 분기(absolute/relative × world/local) 이식.
**타임라인**: `Animation.transform: BTreeMap<String, Vec<Key<TransformMix>>>`(6개 mix).
**op**: `add_transform_constraint`, `set_transform_constraint`(패치), `remove_transform_constraint`, `key_transform`. 이름 변경/삭제 참조 정리(스킨의 `ik` 목록 → `Skin.constraints: Vec<String>`으로 일반화하고 기존 `ik` 필드는 로드 시 마이그레이션).
**UI**: 제약 탭 공통 목록. 인스펙터: 헤더 → "본/타깃" 그룹 → "믹스" 그룹(6개 슬라이더, 오른쪽 요약 "회전 100% · 이동 0%") → "오프셋" 그룹(pair_row 사용) → "모드"(로컬/상대 체크). 뷰포트: 선택 시 본→타깃 점선.

---

### P4. 물리 제약 (가장 큰 구조 변경)

#### 모델
`Document.physics: Vec<PhysicsConstraint>` — 4.2 세대 `PhysicsConstraintData` 대응:
```
name, order, bone: String,
x, y, rotate, scale_x, shear_x: f32,         // 각 속성 영향도 0..1 (0이면 그 속성 시뮬레이션 안 함)
limit: f32 (기본 5000), step: f32 (기본 1/60),
inertia: f32 (1), strength: f32 (100), damping: f32 (1), mass_inverse: f32 (1),
wind: f32 (0), gravity: f32 (0), mix: f32 (1),
inertia_global, strength_global, damping_global, mass_global, wind_global, gravity_global, mix_global: bool
```
`Document.physics_globals`(선택): 전역 wind/gravity 등 — "global" 플래그가 참조할 값. 타임라인: `Animation.physics: BTreeMap<String, Vec<Key<PhysicsFrame>>>`(inertia, strength, damping, mass, wind, gravity, mix 개별 Option) + `physics_reset: Vec<f32>`(리셋 이벤트 시각).

#### 평가 구조
현재 `Pose`는 시간의 순수 함수다. 물리는 이전 프레임 상태가 필요하다.
- 새 타입 `PhysicsState { per_constraint: Vec<BodyState { x, y, vx, vy, rotate, vrotate, scale, vscale, ... , remaining: f32, last_time: f32 }> }`.
- API:
  - `Pose::compute_skinned(doc, tracks, skins)` — **물리 미적용**(셋업/편집 확인용, 기존 호출자 그대로).
  - `Pose::step(doc, tracks, skins, state: &mut PhysicsState, dt: f32) -> Pose` — 고정 스텝(`step`)으로 누적 시간만큼 진행 후 포즈 반환. `PhysicsConstraint.update`의 적분식을 이식(관성은 본 위치 변화량 기반, 바람/중력은 월드 축, `limit`로 속도 제한).
  - `Pose::simulate_to(doc, tracks_fn, skins, t, fps) -> Pose` — **결정적 재현**: 0초부터 `t`까지 `1/fps` 간격으로 `step`을 반복. `tracks_fn(clock)`이 각 시점의 트랙을 만든다. CLI 렌더·MCP·내보내기가 이걸 쓴다(같은 입력 → 같은 출력).
- 적용 위치: 제약 순서(P0-1)에 따라 다른 제약과 섞여 적용.

#### 에디터 동작
- 재생 중: `advance_playback`에서 `PhysicsState`를 유지하며 `Pose::step`. 루프 경계에서 상태 유지.
- 스크럽/정지/키 편집 시: **캐시된 시뮬레이션**을 쓴다. `EditorApp.physics_cache: { key: (doc 버전, anim, layers, skins), frames: Vec<PhysicsState> }` — 0초부터 프레임마다 상태를 저장해 두고, 플레이헤드 이동 시 가장 가까운 이전 프레임에서 이어 계산. 문서가 바뀌면 무효화. (스크럽 시 물리가 튀거나 리셋되지 않도록 한다 — **개선점**)
- 셋업 모드에서 본 드래그: "물리 미리보기" 토글이 켜져 있으면 실시간 `step`(리깅 중 흔들림 확인용). 기본은 꺼짐.
- 상단 모드 스위치 옆에 **물리 컨트롤**: ⏯(시뮬레이션 일시정지), ⟲(리셋), "물리 끄기" 토글. 단축키: Shift+R 리셋.

#### op
`add_physics_constraint`, `set_physics_constraint`(패치), `remove_physics_constraint`, `key_physics`, `key_physics_reset{animation,time}`. 새 제약 기본값은 4.2 세대 기본값과 동일.

#### UI (인스펙터, 물리 아이콘 `assets/icons/physics.png` 이미 생성됨 — `icons.rs`에 미연결)
```
[물리 카드: physics1 · bone: hair_1]
본        [hair_1 ▾]
영향      위치 X [■■■□□] Y [■■■□□]  회전 [■■■■□]  크기 [□□□□□]  셰어 [□□□□□]
── 동작 ─────────────────────── (요약: 관성 1 · 강도 100 · 감쇠 1)
관성      [1.00]   🌐(전역 값 사용 토글)
강도      [100 ]   🌐
감쇠      [1.00]   🌐
질량      [1.00]   🌐
── 환경 ───────────────────────
바람      [0   ]   🌐
중력      [0   ]   🌐
── 고급 ─────────────────────── (접힘)
스텝      [60 Hz]   속도 제한 [5000]   믹스 [1.00]
[▶ 흔들어 보기] (선택 본에 임펄스를 줘서 미리보기)
```
- 각 값 행: 애니메이트 모드에서는 앞에 `key_button`.
- 프리셋 드롭다운(머리카락/천/꼬리/가슴): 값 묶음 적용 — 초보자 접근성 개선, op 1개(`set_physics_constraint`)로 기록.
- 뷰포트: 물리 본은 본 위에 작은 물결 표시, 선택 시 영향 축 화살표.

#### 테스트
- 같은 입력에서 `simulate_to` 결과가 비트 단위로 같음(결정성).
- 강도 0·감쇠 1에서 정지 상태 유지, 중력 >0이면 본이 아래로 처짐(부호 확인).
- 스크럽 캐시: 앞으로/뒤로 이동해도 `simulate_to`와 같은 값.
- CLI `render --anim A --time T`가 물리 포함 결과를 냄(`--no-physics` 플래그로 끌 수 있게).

---

### P5. 블렌드 모드 + 다크 틴트

**모델**: `Slot.blend: BlendMode { normal, additive, multiply, screen }`, `Slot.dark: Option<Color>`(투컬러 틴트). 타임라인: `SlotTimelines.dark: Vec<Key<Color>>`(`rgb2`/`rgba2`).

**렌더러(`skel2d-render`)**: tiny-skia `BlendMode`(Plus/Multiply/Screen)로 매핑. 다크 틴트 식: `out.rgb = (1 - tex.rgb) * dark.rgb * tex.a + tex.rgb * light.rgb` (프리멀티플라이드, 4.2 세대 셰이더와 동일). 픽셀 단위 처리 필요 → 슬롯 그리기를 오프스크린 픽스맵 + 사용자 정의 합성으로.

**에디터 뷰포트 — wgpu 파이프라인으로 전환(§6-1 확정)**: egui 기본 메시 렌더링은 블렌드 모드·투컬러 틴트를 지원하지 않는다. 구현안:
- `egui_wgpu::CallbackTrait`로 **슬롯 전용 wgpu 파이프라인**을 만든다(eframe의 `wgpu` 기능은 이미 켜져 있음). 블렌드 모드별 파이프라인 4개, 투컬러 틴트 셰이더 1개(WGSL).
- 본·기즈모·핸들 오버레이는 지금처럼 egui painter로 그 위에 그린다.
- 이 작업은 `viewport.rs`의 슬롯 그리기 부분(`slot_shapes`, 이미지 텍스처 처리)을 새 모듈 `slot_renderer.rs`로 분리하는 리팩터링을 포함한다.

**UI**: 슬롯 인스펙터 "외형" 그룹 = 색, 다크 색(체크 + 색), 블렌드 드롭다운. 애니메이트 모드에서 각각 키 버튼.

---

### P6. 애니메이션 보강

| 항목 | 설계 |
|---|---|
| 커브 일반화 | `timeline.rs`의 `Target`(Bone/Slot/Ik) 위에 `Channel` 개념 추가(`BoneRotate`, `BoneTranslate`, `SlotColor`, `IkFrame`, `Deform(name)`, `TransformMix`, `Physics…`). `current_bone_key`/`key_curve`/`set_curve`를 `Channel` 기준 제네릭으로. 커브 프리셋 메뉴·베지어 편집기를 모든 연속 채널에 적용. 계단식 채널(attachment, draw order, event, bool)은 커브 UI 숨김. op: `set_curve{animation, target, channel, time, curve}` 신설(지금은 Key*를 다시 써서 바꿈). |
| 키 전체 | K: 선택 항목 키(현행). Shift+K: 선택 본과 모든 자식. Ctrl+Shift+K: 스켈레톤 전체("Key all"). 타임라인 메뉴에도. op는 기존 `key_bone` 여러 개를 한 배치로. |
| 도프시트 행 | 키 없는 항목도 행 표시 옵션: "선택 항목만 / 키 있는 것만 / 전부". 행 우클릭 "플레이헤드에 키". Shift+클릭 빈 셀 = 그 시각에 현재 값 키(힌트 문구에 이미 약속됨). |
| 애니메이션 복제 | op `duplicate_animation{name, new_name}`. 애니 탭 우클릭 + 새 애니메이션 대화상자의 "복제" 체크. |
| X/Y 분리 | 모델 변경 없이 키 값을 `[Option<f32>;2]`로 바꾸지 말고, `BoneTimelines`에 `translate_x/translate_y/scale_x/scale_y`(각 `Vec<Key<f32>>`)를 **추가**한다. 평가 시 결합 타임라인이 있으면 우선, 없으면 분리 타임라인. 인스펙터에서 X만 바꾸면 X 타임라인에만 키(4.x 형식과 동일). 내보내기 시 각각 `translatex/translatey`로. |
| 그래프 에디터 | 타임라인 하단 패널에 "도프시트 / 그래프" 토글. 그래프: 선택 채널의 값 곡선, 키 점·베지어 핸들 직접 드래그, 프레이밍(F). 내부적으로 `curve.rs`의 베지어를 값-시간 공간으로 변환. |
| 드로우 오더 타임라인 | `Animation.draw_order: Vec<DrawOrderKey { time, offsets: Vec<(slot, i32)> }>`(외부 형식과 동일). 애니메이트 모드에서 순서 탭 드래그 = 키 기록. 도프시트 최상단 "드로우 오더" 행. |
| 이벤트 | `Document.events: BTreeMap<String, EventDef { int, float, string, audio, volume, balance }>`, `Animation.events: Vec<EventKey{time, name, 값 오버라이드}>`. 아웃라이너에 이벤트 탭(또는 애니 탭 하위 그룹). 도프시트 "이벤트" 행. CLI `pose`/MCP에 이벤트 발생 목록 출력. |
| ~~스킨 키~~ | **구현하지 않음(§6-2 사용자 결정).** 애니메이션은 스킨을 바꾸지 않는다. |

---

### P7. 어태치먼트 확장

`Attachment` enum에 추가(모두 `name` 보유, 스킨 플레이스홀더로 동작):
- `Point { x, y, rotation, color }` — 총구·이펙트 위치. 뷰포트 십자 표시. 런타임 질의용으로 CLI `pose`에 월드 좌표 출력.
- `BoundingBox { vertices (가중치 가능), color }` — 히트박스. 뷰포트 반투명 다각형, 메쉬 툴과 같은 정점 편집기 재사용.
- `Clipping { vertices, end: String(slot), color }` — 렌더러에서 해당 슬롯부터 `end`까지 클리핑 마스크(tiny-skia `Mask`, wgpu는 스텐실).
- `LinkedMesh { parent: (skin, slot, mesh), image, inherit_timelines: bool }` — 기하·가중치는 원본 공유, 이미지만 교체. 평가 시 원본 메쉬를 해석해 사용. 원본 편집 시 자동 반영.
- `Path { vertices(베지어 제어점), closed, constant_speed, lengths }` + **패스 제약**(`position_mode`, `spacing_mode`, `rotate_mode`, `position`, `spacing`, mixes) — `PathConstraint` 이식. 분량이 크므로 P7 마지막.

UI: 슬롯 우클릭 "새 어태치먼트 ▸ 리전/메쉬/링크드 메쉬/바운딩 박스/포인트/클리핑/패스" → P0-2 대화상자(이름, 초기 모양).

---

### P8. 메쉬 도구 보강
- **수동 엣지**: `Mesh.edges: Vec<[u32;2]>`(제약 엣지). 삼각분할(`meshedit.rs`의 Lawson flip/ear clip)을 제약 들로네로 확장(제약 엣지는 flip 금지). 메쉬 툴: Shift+정점→정점 드래그 = 엣지 추가, 엣지 클릭+Del = 제거. op `set_mesh_edges`.
- **트레이스 옵션**: 알파 임계값, 패딩, 디테일, 오목도 — `skel2d-rig::mesh::OutlineParams` 노출. op `trace_mesh{slot, attachment, alpha, padding, density, keep_weights}`(현재 에디터 전용 `import::trace_mesh`를 코어/rig op로 승격 → AI도 호출 가능).
- **UV 리셋**: 정점을 UV 위치로(리셋해도 가중치 유지).
- 메쉬 툴 창에 모드 버튼(수정/생성/삭제). 우클릭 순환.

---

### P9. 내보내기
- **외부 JSON 4.2**: 새 크레이트 `skel2d-export`. 매핑 표를 모듈 상단 주석에 유지:
  - 본: 회전·셰어·상속 1:1. 슬롯: color/dark/blend. 어태치먼트: region → `region`, mesh → `mesh`(가중치 있으면 4.2 형식의 가중치 정점 배열, `hull` 길이는 외곽 정점 수 계산), linked → `linkedmesh`.
  - 애니메이션: Skel2D 오프셋/배율 → 형식의 값 그대로(셋업 기준), 커브는 베지어 4값.
  - 스킨: 기본 스킨 = 슬롯 자체 어태치먼트, 나머지는 `skins` 배열.
  - Skel2D 전용(키의 `reset`, 레이어 개념) 처리 방식은 **보류(§6-3)**: 착수 전 사용자 확인. 임시로 경고 목록만 반환하고 내보내기는 계속한다.
- **아틀라스**: 사용 이미지 → 맥스렉트 패킹 → `.atlas`(외부 형식) + PNG 페이지. 회전 패킹 옵션.
- **PNG 시퀀스/GIF/비디오**: `skel2d-render`로 프레임 렌더(물리는 `simulate_to`), GIF는 `gif` 크레이트, 비디오는 PATH에 ffmpeg가 있을 때만(없으면 안내).
- CLI: `skel2d export FILE --format skeleton-json|atlas|png-seq|gif|mp4 [--anim A] [--skin K]... -o OUT`. MCP 도구 `skel2d_export`.
- 검증: 내보낸 JSON을 해당 형식의 공식 런타임으로 읽는 테스트는 선택. 최소한 스키마 형태 스냅샷 테스트.

---

## 4. UI/UX 공통 가이드

### 4.1 화면 구성 원칙 (현행 유지 + 확장)
- **선택 항목 하나 = 인스펙터 하나.** 헤더 카드(아이콘·종류·이름·문맥 경로) → 자주 쓰는 값 → 접는 그룹(드물게 쓰는 값은 기본 접힘). 그룹 헤더 오른쪽에 요약을 둬서 접힌 상태로도 값이 보이게.
- **애니메이트 모드에서 값을 바꾸면 키**, 셋업 값은 "셋업 포즈(리그, 키 아님)" 그룹으로 분리하고 툴팁으로 명시. 애니메이트 모드에서 셋업을 몰래 바꾸는 입력은 금지(감사 지적 사항).
- 모든 키 가능한 값 앞에 `key_button`. 색 규칙: 빨강=여기 키, 주황=변경됐지만 미키, 초록=다른 시각에 키(보간), 흐린 초록=키 없음.
- 툴 옵션은 해당 툴이 켜졌을 때 **뷰포트 위 떠 있는 창**(`mesh_tool_window` 패턴). 인스펙터 하단에 묻지 않는다.
- 목록은 전부 `tree_row`: 오른쪽 고정 열(눈·잠금), 보조 정보는 오른쪽 흐린 글씨, 빈 목록은 `empty_state`(아이콘 + 할 일 안내).
- 위험 작업은 팝업 확인 또는 Undo로 되돌릴 수 있음을 상태바로 안내("본 3개 삭제됨 · Ctrl+Z로 취소").

### 4.2 접근성·가독성
- 최소 글자 13px(`SIZE_SMALL`), 본문 15px. 보조 텍스트 대비 7:1 이상(`TEXT_DIM`).
- 색만으로 의미 전달 금지: 키 상태는 색 + 툴팁, 축은 색 + "X/Y" 글자, 오류는 색 + 아이콘/문구.
- 모든 아이콘 버튼에 툴팁(무엇 + 단축키). 모든 기능은 Ctrl+K 팔레트에서 이름으로 실행 가능해야 한다(`palette.rs`에 명령 등록).
- 키보드만으로 조작 가능: 대화상자 Enter/Esc, 아웃라이너 위/아래 화살표 이동(미구현 — P0에서 추가), F2 이름 변경, Del 삭제.
- UI 크기 Ctrl +/-(현행) — 새 위젯은 고정 px 크기 대신 `ui.available_width()`와 `theme` 상수 기반.

### 4.3 의도적인 설계 차이 (유지할 것)
- 스킨 여러 개 동시 미리보기, 미리보기 중 편집이 스킨으로 자동 전환.
- 키 `reset`이 아래 트랙 값으로 돌아감(표정 포즈 위 깜빡임이 자연스럽게 동작).
- 메쉬 재생성·생성·UV 리셋이 가중치를 지우지 않음. 가중치 복사(위치 기반), 미러.
- 미키 변경을 플레이헤드 이동 시 조용히 버리지 않고 상태바로 알림.
- 물리 스크럽 캐시, 셋업 물리 미리보기, 프리셋.

---

## 5. 완료 정의 (각 단계 공통 체크리스트)
- [ ] 모델 필드 + serde 기본값(기존 파일 로드 불변) + 필요 시 마이그레이션 테스트
- [ ] `validate.rs` 검사 + 잘못된 입력 테스트
- [ ] op 추가/패치, 이름 변경·삭제 시 참조 정리 테스트
- [ ] 포즈/렌더 반영 + 수치 테스트(가능하면 4.2 런타임 동작과 같은 기대값)
- [ ] CLI/HTTP/MCP 노출(렌더·pose 파라미터 포함) + `docs/AI_GUIDE.md` 갱신
- [ ] 에디터 UI(인스펙터·아웃라이너·도프시트·팔레트·단축키 도움말) + 한국어 번역
- [ ] 아이콘 필요 시 생성(64×64 흰색 글리프) 후 `icons.rs` 연결
- [ ] `cargo test --workspace --release` 통과, `cargo clippy --workspace --release` 경고 0
- [ ] 스크린샷으로 실제 화면 확인(셋업/애니메이트 모드 각각)

## 6. 사용자 결정 (2026-10-30 확정)
1. **P5 뷰포트: wgpu 커스텀 파이프라인으로 전환한다.** 근사 표시안은 폐기. §P5의 권장안대로 구현.
2. **스킨 키(애니메이션에서 스킨 전환): 도입하지 않는다.** §P6 표의 "스킨 키" 항목은 구현하지 말 것. 스킨 전환은 런타임/미리보기에서만.
3. **외부 JSON 내보내기에서 Skel2D 전용 개념 처리: 보류.** P9 착수 전 사용자에게 다시 확인. 그 전까지는 해당 개념(키 `reset`, 다중 스킨 동시 적용 등)을 만나면 내보내기를 중단하지 말고 경고 목록만 반환하는 임시 동작으로 둔다.
4. **첫 커밋·브랜치 전략: 보류.** 사용자 지시 전까지 커밋하지 말 것. 대신 큰 변경 전 `crates/`, `docs/`, `examples/`를 임시 폴더로 백업한다(§0.3).
