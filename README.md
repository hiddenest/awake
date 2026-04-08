# awake

> AI 코딩 에이전트가 **실제로 작업 중일 때**만 macOS가 잠들지 않도록 `caffeinate`와 `pmset -c disablesleep`를 함께 관리하는 bash 스크립트

## 요구사항

- macOS
- bash 3.2 이상 (macOS 기본 `/bin/bash` 호환)
- lid-close sleep 방지를 쓰려면 `pmset -c disablesleep`를 실행할 권한이 있어야 함

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

# 디스플레이만 유지하도록 시작
awake start -D &

# 상태 확인
awake status

# 중지
awake stop
```

`awake start`와 `awake install`은 같은 옵션을 받습니다.

- `-D`, `-d`, `--display` — 활성 상태일 때 디스플레이가 꺼지지 않도록 유지
- `-i`, `--idle-system` — 활성 상태일 때 유휴 시스템 잠자기 방지

옵션을 생략하면 기본값으로 `caffeinate -di`를 사용합니다.

실제 작업 중에는 `caffeinate -di`와 함께 `sudo pmset -c disablesleep 1`도 시도합니다. 작업이 끝나거나 `awake`가 종료되면 원래 `SleepDisabled` 값으로 복구합니다.
권한 모델은 두 가지를 지원합니다.

- `awake` 자체를 root로 실행하면 `pmset`을 직접 호출합니다.
- 일반 사용자로 실행하면 `sudo -n pmset ...`를 사용합니다.

비대화형 `sudo`가 불가능하고 root로도 실행하지 않으면 `pmset` 토글은 건너뛰고 경고만 출력합니다.

### `pmset` 권한 부여 방법

#### 1. 권장: `pmset`만 NOPASSWD 허용

`visudo`로 다음과 같은 sudoers 규칙을 추가합니다.

```sudoers
your_username ALL=(root) NOPASSWD: /usr/bin/pmset
```

이렇게 하면 `awake`는 일반 사용자로 실행되면서도 `pmset -c disablesleep`만 비밀번호 없이 실행할 수 있습니다.

#### 2. 대안: `awake`를 root로 실행

```bash
sudo ./awake start
```

이 방법은 동작은 단순하지만, 스크립트 전체가 root 권한으로 실행되므로 권한 범위가 더 큽니다.

### LaunchAgent (자동 시작)

```bash
# LaunchAgent 설치 (로그인 시 자동 시작)
awake install

# LaunchAgent를 디스플레이 유지 모드로 설치
awake install -D

# LaunchAgent 제거
awake uninstall
```

## 감시 대상 프로세스

awake는 다음 프로세스명을 감시합니다.

- `claude` — Claude Code CLI
- `codex` — OpenAI Codex CLI
- `opencode` — OpenCode CLI
- `opencode-cli` — OpenCode CLI (별칭)
- `pi` — Pi Coding Agent CLI

프로세스가 감지되면 5초마다 활동 신호를 확인합니다. `codex app-server`, `opencode serve`처럼 지속적으로 떠 있는 **서버형 프로세스**는 **직계 자식 프로세스 수 증가**만 작업 신호로 보고, 그 외 일반 CLI 프로세스는 **직계 자식 프로세스 수 증가** 또는 **CPU 사용 시간 증가(0.01초 이상)**를 작업 신호로 봅니다. 같은 이름의 프로세스가 여러 개 있으면 그중 **하나라도 활동 신호를 보일 때만** 작업 중으로 판단합니다. 작업 중에는 per-target `caffeinate`를 유지하고, 활성 타깃이 하나라도 있으면 전역적으로 `pmset -c disablesleep 1`도 시도합니다.
새 PID가 처음 보였다는 이유만으로 바로 활성으로 보지 않으며, 활동 신호가 3회 연속(15초) 없으면 **유휴 상태**로 판단하여 caffeinate를 해제합니다.
작업이 재개되면 caffeinate가 자동으로 다시 활성화됩니다.

## 제한사항

Codex CLI는 Node.js 기반으로 동작하기 때문에 실제 프로세스명이 `node`로 표시됩니다. `pgrep -x codex`로는 감지되지 않을 수 있습니다. 이 경우 TARGETS 배열에 `node`를 추가하면 되지만, 다른 Node.js 프로세스도 함께 감지될 수 있습니다.

## 동작 원리

1. `awake start` 실행 시 PID 파일(`/tmp/awake.pid`)을 생성하고 폴링 루프 진입
2. 5초마다 TARGETS 목록의 프로세스를 `pgrep -x`로 확인
3. 프로세스 감지 시 모든 matching PID의 직계 자식 프로세스 수와 `ps -o cputime` 값을 측정
4. `codex app-server`, `opencode serve` 같은 서버형 프로세스는 직계 자식 증가가 있을 때만 **활성** 상태로 판단
5. 그 외 일반 CLI 프로세스는 직계 자식 증가 또는 CPU 사용 시간이 0.01초 이상 증가하면 **활성** 상태로 판단 → 선택된 옵션으로 `caffeinate` 실행 (기본값 `-di`)
6. 활성 타깃이 하나라도 있으면 전역적으로 `pmset -c disablesleep 1`을 시도 (root 실행 또는 `sudo -n` 경로)
7. 활동 신호가 3회 연속(15초) 없으면 **유휴** 상태로 판단 → caffeinate 해제
8. 활성 타깃이 모두 없어지면 `pmset`을 원래 `SleepDisabled` 값으로 복구
9. 활성 상태로 돌아오면 caffeinate / pmset 재활성화
10. 프로세스 종료 시 caffeinate 즉시 해제
11. `awake stop` 또는 SIGTERM 수신 시 모든 caffeinate 종료 후 PID 파일 삭제와 함께 `pmset` 복구 시도

## 디버깅

caffeinate가 실제로 동작 중인지 확인하려면 다음 명령어를 사용합니다.

```bash
# 현재 활성화된 power assertions 확인
pmset -g assertions

# awake 상태 확인
awake status

# caffeinate 프로세스 직접 확인
pgrep -a caffeinate

# 현재 SleepDisabled 값 확인
pmset -g | grep SleepDisabled
```

`pmset -g assertions` 출력에서 `PreventUserIdleDisplaySleep` 또는 `PreventUserIdleSystemSleep` 항목이 있으면 caffeinate가 정상 동작 중입니다.
