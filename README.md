# awake

> AI 코딩 에이전트가 실행 중일 때 macOS가 잠들지 않도록 자동으로 `caffeinate`를 실행하는 bash 스크립트

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

현재 `install` / `uninstall` 서브커맨드는 미구현 상태입니다. 수동으로 LaunchAgent plist를 작성하여 등록할 수 있습니다.

```bash
# ~/Library/LaunchAgents/com.awake.plist 생성 후
launchctl load ~/Library/LaunchAgents/com.awake.plist

# 제거
launchctl unload ~/Library/LaunchAgents/com.awake.plist
```

## 감시 대상 프로세스

awake는 다음 프로세스명을 감시합니다.

- `claude` — Claude Code CLI
- `codex` — OpenAI Codex CLI

프로세스가 감지되면 `caffeinate -di -w <PID>`를 실행하여 디스플레이 및 시스템 idle sleep을 방지합니다. 프로세스가 종료되면 caffeinate도 자동으로 해제됩니다.

## 제한사항

Codex CLI는 Node.js 기반으로 동작하기 때문에 실제 프로세스명이 `node`로 표시됩니다. `pgrep -x codex`로는 감지되지 않을 수 있습니다. 이 경우 TARGETS 배열에 `node`를 추가하면 되지만, 다른 Node.js 프로세스도 함께 감지될 수 있습니다.

## 동작 원리

1. `awake start` 실행 시 PID 파일(`/tmp/awake.pid`)을 생성하고 폴링 루프 진입
2. 5초마다 TARGETS 목록의 프로세스를 `pgrep -x`로 확인
3. 프로세스 감지 시 `caffeinate -di -w <PID>` 백그라운드 실행
4. 프로세스 종료 시 해당 caffeinate 프로세스 종료
5. `awake stop` 또는 SIGTERM 수신 시 모든 caffeinate 종료 후 PID 파일 삭제

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
