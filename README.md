# Discord Audio Binder

![앱 아이콘](assets/app-icon.png)

Windows에서 선택한 게임 창을 별도의 `GameOutput` 창으로 출력하고, 게임 오디오와 TIDAL 오디오를 Discord 창 공유에 함께 실을 수 있도록 만든 도구입니다.

이 프로젝트는 AI를 활용해 제작되었습니다. 소스 코드와 빌드 결과물은 누구나 허락 없이 사용, 복사, 수정, 재배포할 수 있습니다. 자세한 조건은 [LICENSE](LICENSE)를 확인하세요.

## 주요 기능

- Windows Graphics Capture를 이용한 특정 창 캡처
- 제목 표시줄 제외 및 사용자 지정 화면 비율 크롭
- 크롭 영역 왼쪽, 중앙, 오른쪽 정렬
- 30~180 FPS 출력과 VSync 설정
- HDR 화면의 SDR 톤 매핑
- 선택한 게임 프로세스의 오디오를 앱에서 다시 출력
- VB-CABLE 등 별도 출력 장치를 이용한 게임 소리 중복 청취 방지
- 내장 WebView2 창을 통한 TIDAL 및 기타 웹 오디오 재생
- 게임 오디오와 TIDAL 오디오 개별 활성화

## 사용 방법

1. [Releases](../../releases)에서 최신 `discord-audio-binder.exe`를 받습니다.
2. 앱을 실행하고 캡처할 게임 창을 선택합니다.
3. 필요한 크롭 비율, 출력 FPS, VSync 설정을 조정합니다.
4. 게임 오디오 송출이 필요하면 출력 장치를 선택합니다. 게임 소리가 두 번 들리면 VB-CABLE처럼 직접 듣지 않는 장치를 선택할 수 있습니다.
5. Discord에서 애플리케이션 공유 대상으로 `GameOutput` 창을 선택합니다.

TIDAL 기능을 사용하려면 Microsoft Edge WebView2 Runtime이 설치되어 있어야 합니다.

## 직접 빌드

Rust stable과 Windows SDK가 설치된 Windows 환경에서 다음 명령을 실행합니다.

```powershell
cargo build --release
```

결과물은 `target\release\discord-audio-binder.exe`에 생성됩니다.

## 참고 사항

- Windows 전용 애플리케이션입니다.
- Discord, Windows 또는 스트리밍 서비스의 업데이트에 따라 캡처 동작이 달라질 수 있습니다.
- 이 소프트웨어는 어떠한 보증도 없이 제공됩니다. 사용에 따른 책임은 사용자에게 있습니다.

## 라이선스

[The Unlicense](LICENSE). 상업적 이용을 포함해 용도 제한 없이 자유롭게 사용할 수 있습니다.
