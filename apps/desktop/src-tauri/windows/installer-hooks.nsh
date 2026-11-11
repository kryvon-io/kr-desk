; Ganchos do instalador NSIS (Tauri): serviço do KR Desk.
;
; O instalador oferece dois modos:
; - "Só para mim" (padrão): sem administrador e sem serviço, como até a 0.7.
; - "Para todos os usuários": em Arquivos de Programas (só administradores
;   escrevem lá) e com o serviço, que roda como SYSTEM a partir de $INSTDIR.

!macro NSIS_HOOK_PREINSTALL
  ${If} $MultiUser.InstallMode == "AllUsers"
    ; Atualização: para o serviço (e o agente) antes de trocar o executável.
    nsExec::Exec 'sc.exe stop KRDeskService'
    Pop $0
    Sleep 2000
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $MultiUser.InstallMode == "AllUsers"
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --install-service' $0
    ${If} $0 != 0
      DetailPrint "Não foi possível instalar o serviço do KR Desk (código $0)."
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $MultiUser.InstallMode == "AllUsers"
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-service' $0
  ${EndIf}
!macroend
