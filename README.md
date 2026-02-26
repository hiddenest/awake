# awake

> AI 코딩 에이전트가 **실제로 작업 중일 때**만 macOS가 잠들지 않도록 자동으로 `caffeinate`를 실행하는 bash 스크립트

## 요구사항

- macOS
- bash 3.2 이상 (macOS 기본 `/bin/bash` 호환)

## 빠른 시작

```bash
# 저장소 클론
git clone <repo-url>
cd experiments-claude-code-awake

# 실행 권한 부여
chmod +x awake

# PATH에 추가 (선택)
sudo cp awake /usr/local/bin/awake
```

## 사용법

### 수동 실행

```bash
# 백그라운드에서 시작
awake start &

# 상태 확인
awake status

# 중지
awake stop
```

### LaunchAgent (자동 시작)

```bash
# LaunchAgent 설치 (로그인 시 자동 시작)
awake install

# LaunchAgent 제거
awake uninstall
```

## 감시 대상 프로세스

awake는 다음 프로세스명을 감시합니다.

- `claude` — Claude Code CLI
- `codex` — OpenAI Codex CLI
- `opencode` — OpenCode CLI
- `opencode-cli` — OpenCode CLI (별칭)

프로세스가 감지되면 CPU 사용 시간을 5초마다 측정합니다. CPU 시간이 변화하면 **작업 중**으로 판단하여 `caffeinate -di`를 실행합니다.
CPU 시간이 3회 연속(15초) 변화하지 않으면 **유휴 상태**로 판단하여 caffeinate를 해제합니다.
작업이 재개되면 caffeinate가 자동으로 다시 활성화됩니다.

## 제한사항

Codex CLI는 Node.js 기반으로 동작하기 때문에 실제 프로세스명이 `node`로 표시됩니다. `pgrep -x codex`로는 감지되지 않을 수 있습니다. 이 경우 TARGETS 배열에 `node`를 추가하면 되지만, 다른 Node.js 프로세스도 함께 감지될 수 있습니다.

## 동작 원리

1. `awake start` 실행 시 PID 파일(`/tmp/awake.pid`)을 생성하고 폴링 루프 진입
2. 5초마다 TARGETS 목록의 프로세스를 `pgrep -x`로 확인
3. 프로세스 감지 시 `ps -o cputime`으로 CPU 사용 시간 측정
4. CPU 시간이 이전 측정값과 다르면 **활성** 상태로 판단 → `caffeinate -di` 실행
5. CPU 시간이 3회 연속(15초) 동일하면 **유휴** 상태로 판단 → caffeinate 해제
6. 활성 상태로 돌아오면 caffeinate 재활성화
7. 프로세스 종료 시 caffeinate 즉시 해제
8. `awake stop` 또는 SIGTERM 수신 시 모든 caffeinate 종료 후 PID 파일 삭제

## 디버깅

caffeinate가 실제로 동작 중인지 확인하려면 다음 명령어를 사용합니다.

```bash
# 현재 활성화된 power assertions 확인
pmset -g assertions

# awake 상태 확인
awake status

# caffeinate 프로세스 직접 확인
pgrep -a caffeinate
```

`pmset -g assertions` 출력에서 `PreventUserIdleDisplaySleep` 또는 `PreventUserIdleSystemSleep` 항목이 있으면 caffeinate가 정상 동작 중입니다.
