#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }
# Wire-format tests for starship-daemon.psm1. No daemon needed:
# STARSHIP_DAEMON_NO_AUTOSTART suppresses module autostart during import.
BeforeAll {
    $env:STARSHIP_DAEMON_NO_AUTOSTART = '1'
    Import-Module (Join-Path $PSScriptRoot "..\starship-daemon.psm1") -Force

    function New-Resp([string]$Text) {
        $p = [Text.Encoding]::UTF8.GetBytes($Text)
        $f = [byte[]]::new(4 + $p.Length)
        [BitConverter]::GetBytes([uint32]$p.Length).CopyTo($f, 0)
        [Buffer]::BlockCopy($p, 0, $f, 4, $p.Length)
        ,$f
    }
}

Describe 'StarshipFrame wire format' {
    It 'Build emits golden bytes matching Rust encode_request' {
        # kind=2 (REQ_TIMINGS), cwd '/r', status -3, keymap 'vi', width 120,
        # config none, cache enabled. Cross-checked against src/lib.rs encode_request.
        $want = [byte[]](
            0x02,
            0x15, 0x00, 0x00, 0x00,
            0x02, 0x00, 0x00, 0x00, 0x2F, 0x72,
            0xFD, 0xFF, 0xFF, 0xFF,
            0x02, 0x00, 0x76, 0x69,
            0x78, 0x00, 0x00, 0x00,
            0x00, 0x00,
            0x00
        )
        $actual = [StarshipFrame]::Build('/r', -3, 'vi', 120, $null, [byte]0, [byte]2)
        # No comma-wrap: Pester v5 collects piped items, so the unrolled bytes
        # arrive as one 26-element collection and compare elementwise.
        $actual | Should -Be $want
    }

    It 'Build defaults kind to REQ_PROMPT (1)' {
        $req = [StarshipFrame]::Build('/repo', 7, 'vi', 100, $null, [byte]0)
        $req[0] | Should -Be 1
    }

    It 'Parse roundtrips a Build-sized response' {
        $resp = New-Resp 'prompt>'
        [StarshipFrame]::Parse($resp, $resp.Length) | Should -Be 'prompt>'
    }

    It 'Parse rejects garbage frames' {
        [StarshipFrame]::Parse([byte[]](1, 2), 2) | Should -BeNullOrEmpty
        [StarshipFrame]::Parse([byte[]](0, 0, 0, 0, 65), 5) | Should -BeNullOrEmpty
        $truncated = (New-Resp 'hi')[0..3]
        [StarshipFrame]::Parse($truncated, $truncated.Length) | Should -BeNullOrEmpty
    }
}

Describe 'Get-StarshipRespBuf' {
    It 'returns a 64KB byte[], not an unrolled Object[]' {
        # Internal function: invoke inside the module's session state.
        $buf = & (Get-Module starship-daemon) { Get-StarshipRespBuf }
        ($buf -is [byte[]]) | Should -BeTrue
        $buf.Length | Should -Be 65536
    }
}

Describe 'multibyte cwd encoding' {
    It 'encodes cwd length in UTF-8 bytes, not chars' {
        # '/repo/' = 6 ASCII bytes + 9 UTF-8 bytes = 15
        $req = [StarshipFrame]::Build("/repo/$([char]0x65E5)$([char]0x672C)$([char]0x8A9E)", 0, '', 80, $null, [byte]0)
        [BitConverter]::ToUInt32($req, 5) | Should -Be 15
    }
}
