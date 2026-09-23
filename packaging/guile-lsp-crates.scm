;;; Crate sources for building guile-lsp with Guix's cargo-build-system.
;;; Generated from Cargo.lock with
;;;   guix import crate -f Cargo.lock guile-lsp
;;; followed by the lookup table at the end; regenerate both after
;;; changing dependencies. See guix.scm at the repository root.

(define-module (packaging guile-lsp-crates)
  #:use-module (guix build-system cargo)
  #:export (lookup-cargo-inputs))

(define rust-async-trait-0.1.92
  (crate-source "async-trait" "0.1.92"
                "0rqn5iga1hlv2lm8xzav1zhar46jb4dvx89i6kfv93kb53maxxl2"))

(define rust-auto-impl-1.3.0
  (crate-source "auto_impl" "1.3.0"
                "1rzysfb0zpwsi2lg043fj596xj2a4xir2lc0fd17im64vc5vgp7z"))

(define rust-bitflags-1.3.2
  (crate-source "bitflags" "1.3.2"
                "12ki6w8gn1ldq7yz9y680llwk5gmrhrzszaa17g1sbrw2r2qvwxy"))

(define rust-bitflags-2.13.2
  (crate-source "bitflags" "2.13.2"
                "01hbgjwvid66850fzi76mvn5f2bqycx6sf165ng1kfjqq9bl1v9x"))

(define rust-bytes-1.12.1
  (crate-source "bytes" "1.12.1"
                "017z19dpg4f942h051m7bpnzcgng042hhcpd7bmg7bjjqd42lrgw"))

(define rust-cfg-if-1.0.5
  (crate-source "cfg-if" "1.0.5"
                "0026j56901nzjraap3da0a8njw42j66zcxnn6s2s9aa5bcblhxjf"))

(define rust-dashmap-5.5.3
  (crate-source "dashmap" "5.5.3"
                "0miqnlxi501vfbv6mw5jbmzgnj0wjrch3p4abvpd59s9v30lg1wp"))

(define rust-displaydoc-0.2.7
  (crate-source "displaydoc" "0.2.7"
                "1a42mwpgpwcqq2qqgkcc630wvsc2p2dkmgacjnclginwfz9js8y6"))

(define rust-errno-0.3.14
  (crate-source "errno" "0.3.14"
                "1szgccmh8vgryqyadg8xd58mnwwicf39zmin3bsn63df2wbbgjir"))

(define rust-form-urlencoded-1.2.2
  (crate-source "form_urlencoded" "1.2.2"
                "1kqzb2qn608rxl3dws04zahcklpplkd5r1vpabwga5l50d2v4k6b"))

(define rust-futures-0.3.34
  (crate-source "futures" "0.3.34"
                "18yhwmbdalhz2z9i1vm10hy2v0cfm82dkgcb6vr2msxazfix4ccs"))

(define rust-futures-channel-0.3.34
  (crate-source "futures-channel" "0.3.34"
                "1i4kwcanpaphn1ax62ci3nx176kglxqx0gnhzqpqdr1rkpbf7ydi"))

(define rust-futures-core-0.3.34
  (crate-source "futures-core" "0.3.34"
                "0pjgv4fx0np6hrs5sz5a2phabwv0z70yr51v03injbi44bjrkmlj"))

(define rust-futures-io-0.3.34
  (crate-source "futures-io" "0.3.34"
                "1v9z6wj92ra18kpv0xig21hgpzrvcwmcr8fszyzh64yyay0zmh2k"))

(define rust-futures-macro-0.3.34
  (crate-source "futures-macro" "0.3.34"
                "0i0czvcvsqq4hrccibq2f23004si5z34zjwdxfmqhlrmm15nbfcz"))

(define rust-futures-sink-0.3.34
  (crate-source "futures-sink" "0.3.34"
                "07cfvrgc3vxk6sw5g8a8dnrm1mzg6d5mwy08ywa1sgyhyxml4i0r"))

(define rust-futures-task-0.3.34
  (crate-source "futures-task" "0.3.34"
                "1zfilqs8nwlfqz4prk7ihvpp5avvzins87ibzlxzq5fhs7ipshfd"))

(define rust-futures-util-0.3.34
  (crate-source "futures-util" "0.3.34"
                "1g3r9ghzq7c2fh34lis43i72xavk9p84npgfwgb5vfpqcwjajl0d"))

(define rust-hashbrown-0.14.5
  (crate-source "hashbrown" "0.14.5"
                "1wa1vy1xs3mp11bn3z9dv0jricgr6a2j0zkf1g19yz3vw4il89z5"))

(define rust-httparse-1.10.1
  (crate-source "httparse" "1.10.1"
                "11ycd554bw2dkgw0q61xsa7a4jn1wb1xbfacmf3dbwsikvkkvgvd"))

(define rust-icu-collections-2.3.0
  (crate-source "icu_collections" "2.3.0"
                "04x59h6vdq0cnpippim1nr471ivlsnnn470sj1d5v864h48d4s7s"))

(define rust-icu-locale-core-2.3.0
  (crate-source "icu_locale_core" "2.3.0"
                "1sqdj16wwl7h9y6r7j394av4kpdb7zryz9h169ffwbm9imc2hvnm"))

(define rust-icu-normalizer-2.3.0
  (crate-source "icu_normalizer" "2.3.0"
                "0vv43ixk2wmbxrx7kl33cwkhx1wdyb1q3pa18qkyshan4dgwzy8j"))

(define rust-icu-normalizer-data-2.3.0
  (crate-source "icu_normalizer_data" "2.3.0"
                "1811h0ppb7lwq1q2492p5x6lcmlwmhbmkf69fhyvzcz0scgdlqqm"))

(define rust-icu-properties-2.3.0
  (crate-source "icu_properties" "2.3.0"
                "0j51hi8qgf0l6a7qzvnwsc61598w2fpnsklicld6ci9immva4z3y"))

(define rust-icu-properties-data-2.3.0
  (crate-source "icu_properties_data" "2.3.0"
                "1akw1gp5rcaiz377xzsnkx8f92qax4kh3lfn9y4rcjj6q4wg1475"))

(define rust-icu-provider-2.3.1
  (crate-source "icu_provider" "2.3.1"
                "0wrydhwprwgyka3r3sw6276syjnlw6fpqr2zsm2srvxv7afvnyyj"))

(define rust-idna-1.1.0
  (crate-source "idna" "1.1.0"
                "1pp4n7hppm480zcx411dsv9wfibai00wbpgnjj4qj0xa7kr7a21v"))

(define rust-idna-adapter-1.2.2
  (crate-source "idna_adapter" "1.2.2"
                "0557p76l8hj35r9zn1yv7c6x1c0qbrsffmg80n0yy8361ly3fs6b"))

(define rust-itoa-1.0.18
  (crate-source "itoa" "1.0.18"
                "10jnd1vpfkb8kj38rlkn2a6k02afvj3qmw054dfpzagrpl6achlg"))

(define rust-libc-0.2.189
  (crate-source "libc" "0.2.189"
                "1whjfs375vlng2q6yrbzs73cvp5lm3w1n2gfqajb2vgf7zg3xbry"))

(define rust-litemap-0.8.3
  (crate-source "litemap" "0.8.3"
                "1bpgpj87560hmckh3875fbahpmfxbk4g8pzns84h3ykf3nfx3na7"))

(define rust-lock-api-0.4.14
  (crate-source "lock_api" "0.4.14"
                "0rg9mhx7vdpajfxvdjmgmlyrn20ligzqvn8ifmaz7dc79gkrjhr2"))

(define rust-lsp-types-0.94.1
  (crate-source "lsp-types" "0.94.1"
                "18aym1qfgyaf9f9504yz911gssck5rv190izzr3hdqbal12gssy6"))

(define rust-memchr-2.8.3
  (crate-source "memchr" "2.8.3"
                "161xa63ipfanf8v3nb82xd5hqgydv55nzw59wyngqbz6alfaz2yg"))

(define rust-mio-1.2.3
  (crate-source "mio" "1.2.3"
                "1n5ryp7j5fga38z7php5yy9k7ia24rp6cl9gm27zwar6khz4862b"))

(define rust-once-cell-1.21.4
  (crate-source "once_cell" "1.21.4"
                "0l1v676wf71kjg2khch4dphwh1jp3291ffiymr2mvy1kxd5kwz4z"))

(define rust-parking-lot-core-0.9.12
  (crate-source "parking_lot_core" "0.9.12"
                "1hb4rggy70fwa1w9nb0svbyflzdc69h047482v2z3sx2hmcnh896"))

(define rust-percent-encoding-2.3.2
  (crate-source "percent-encoding" "2.3.2"
                "083jv1ai930azvawz2khv7w73xh8mnylk7i578cifndjn5y64kwv"))

(define rust-pin-project-1.1.13
  (crate-source "pin-project" "1.1.13"
                "09091qp946lpmjz4yp0xil1r5v4hgc91fi19dg5csayhdqrv4ri4"))

(define rust-pin-project-internal-1.1.13
  (crate-source "pin-project-internal" "1.1.13"
                "12rzlh07i1sdgrvzj6wgkka5bjqyvbfsl8knq6qi7g16m7q9aqy9"))

(define rust-pin-project-lite-0.2.17
  (crate-source "pin-project-lite" "0.2.17"
                "1kfmwvs271si96zay4mm8887v5khw0c27jc9srw1a75ykvgj54x8"))

(define rust-potential-utf-0.1.6
  (crate-source "potential_utf" "0.1.6"
                "0qbndl2fpphq7mph41m11vaixs05xrh1s451wxlgap4fdnybjgnq"))

(define rust-proc-macro2-1.0.107
  (crate-source "proc-macro2" "1.0.107"
                "1nb6ly8kp65f724kj73ippc7lvydss24sm2vagk6qpklpg4pwplq"))

(define rust-quote-1.0.47
  (crate-source "quote" "1.0.47"
                "00ch0yyzvv6s671ik0kcsbw8nigdaj2g3fr61kcahwx48aqlvgqz"))

(define rust-redox-syscall-0.5.18
  (crate-source "redox_syscall" "0.5.18"
                "0b9n38zsxylql36vybw18if68yc9jczxmbyzdwyhb9sifmag4azd"))

(define rust-scopeguard-1.2.0
  (crate-source "scopeguard" "1.2.0"
                "0jcz9sd47zlsgcnm1hdw0664krxwb5gczlif4qngj2aif8vky54l"))

(define rust-serde-1.0.229
  (crate-source "serde" "1.0.229"
                "1fp04fq4a79bpm61xz1zy0pbz4kpc7d771zii1k3inmszq55jj21"))

(define rust-serde-core-1.0.229
  (crate-source "serde_core" "1.0.229"
                "0j1ajiha76h3nmd976il9li6975k121xa7jb39ws8n0yqp4s5p37"))

(define rust-serde-derive-1.0.229
  (crate-source "serde_derive" "1.0.229"
                "0j4k63i7h1bikxwz2c89ig0hrwbnl9mz1czn85xx99x5cc9dg9g7"))

(define rust-serde-json-1.0.151
  (crate-source "serde_json" "1.0.151"
                "051zww7lvpw147vvwss1ng6w587qyrkzg75fvj08q2dfrmgbahf8"))

(define rust-serde-repr-0.1.21
  (crate-source "serde_repr" "0.1.21"
                "01l987ghc17h1y9cf9xbzmcs77575mbrjf4ca2h70g15vqlicfwd"))

(define rust-signal-hook-registry-1.4.8
  (crate-source "signal-hook-registry" "1.4.8"
                "06vc7pmnki6lmxar3z31gkyg9cw7py5x9g7px70gy2hil75nkny4"))

(define rust-slab-0.4.12
  (crate-source "slab" "0.4.12"
                "1xcwik6s6zbd3lf51kkrcicdq2j4c1fw0yjdai2apy9467i0sy8c"))

(define rust-smallvec-1.16.1
  (crate-source "smallvec" "1.16.1"
                "14gqvsqdli51r1bii3hfqv5vx1b9r0gic4br0x9fsixmy5b70ims"))

(define rust-stable-deref-trait-1.2.1
  (crate-source "stable_deref_trait" "1.2.1"
                "15h5h73ppqyhdhx6ywxfj88azmrpml9gl6zp3pwy2malqa6vxqkc"))

(define rust-syn-2.0.119
  (crate-source "syn" "2.0.119"
                "15vjy620l91a3q4n4f4gzhnflmdr6pnm38v2m6cpk86i8av32a47"))

(define rust-syn-3.0.6
  (crate-source "syn" "3.0.6"
                "1vmw7s58rzrs926nv5m06x7qbgswm1aa9iw3s1bj5var47kyi4w5"))

(define rust-synstructure-0.14.0
  (crate-source "synstructure" "0.14.0"
                "00kzyz38xqvs7x1dn6rp64y0ckly4m7ywf3qk3qkgqfzs3nh85wh"))

(define rust-tinystr-0.8.4
  (crate-source "tinystr" "0.8.4"
                "0hzncw8rgk4syla79qscfml46jm7ll1zdp7kdacc42cj8n8prqmi"))

(define rust-tokio-1.53.1
  (crate-source "tokio" "1.53.1"
                "1v8b3b45pkpbibls75yniqbvx5dlks2708141ljni5mnf6lawb10"))

(define rust-tokio-macros-2.7.2
  (crate-source "tokio-macros" "2.7.2"
                "03kvy2r5gr4zccm4vdx8vvv3q69kbjc1b006rs11aibz74m3lxvq"))

(define rust-tokio-util-0.7.19
  (crate-source "tokio-util" "0.7.19"
                "0licqrhrawysjrsr0qw3cgzkkjph7090hlcqcm45aazmkg81aj29"))

(define rust-tower-0.4.13
  (crate-source "tower" "0.4.13"
                "073wncyqav4sak1p755hf6vl66njgfc1z1g1di9rxx3cvvh9pymq"))

(define rust-tower-layer-0.3.3
  (crate-source "tower-layer" "0.3.3"
                "03kq92fdzxin51w8iqix06dcfgydyvx7yr6izjq0p626v9n2l70j"))

(define rust-tower-lsp-0.20.0
  (crate-source "tower-lsp" "0.20.0"
                "0245266rj84zgd4wv6nsnxcq7nkydqblrhxkv4l7cqm6ahmhbfnl"))

(define rust-tower-lsp-macros-0.9.0
  (crate-source "tower-lsp-macros" "0.9.0"
                "1yi2ahsndbb1xkw05dx95237bq9lq26i0h7ly8klp6hb9qnr1zc4"))

(define rust-tower-service-0.3.3
  (crate-source "tower-service" "0.3.3"
                "1hzfkvkci33ra94xjx64vv3pp0sq346w06fpkcdwjcid7zhvdycd"))

(define rust-tracing-0.1.44
  (crate-source "tracing" "0.1.44"
                "006ilqkg1lmfdh3xhg3z762izfwmxcvz0w7m4qx2qajbz9i1drv3"))

(define rust-tracing-attributes-0.1.31
  (crate-source "tracing-attributes" "0.1.31"
                "1np8d77shfvz0n7camx2bsf1qw0zg331lra0hxb4cdwnxjjwz43l"))

(define rust-tracing-core-0.1.36
  (crate-source "tracing-core" "0.1.36"
                "16mpbz6p8vd6j7sf925k9k8wzvm9vdfsjbynbmaxxyq6v7wwm5yv"))

(define rust-unicode-ident-1.0.26
  (crate-source "unicode-ident" "1.0.26"
                "0m3915ipi4zz7isncf5k1dz47ys0nq9j7l4l2n2rm03zaxwg8ifj"))

(define rust-url-2.5.8
  (crate-source "url" "2.5.8"
                "1v8f7nx3hpr1qh76if0a04sj08k86amsq4h8cvpw6wvk76jahrzz"))

(define rust-utf8-iter-1.0.4
  (crate-source "utf8_iter" "1.0.4"
                "1gmna9flnj8dbyd8ba17zigrp9c4c3zclngf5lnb5yvz1ri41hdn"))

(define rust-wasi-0.11.1+wasi-snapshot-preview1
  (crate-source "wasi" "0.11.1+wasi-snapshot-preview1"
                "0jx49r7nbkbhyfrfyhz0bm4817yrnxgd3jiwwwfv0zl439jyrwyc"))

(define rust-windows-link-0.2.1
  (crate-source "windows-link" "0.2.1"
                "1rag186yfr3xx7piv5rg8b6im2dwcf8zldiflvb22xbzwli5507h"))

(define rust-windows-sys-0.61.2
  ;; TODO REVIEW: Check bundled sources.
  (crate-source "windows-sys" "0.61.2"
                "1z7k3y9b6b5h52kid57lvmvm05362zv1v8w0gc7xyv5xphlp44xf"))

(define rust-writeable-0.6.4
  (crate-source "writeable" "0.6.4"
                "1p3r4s4wbf3dksfpj3xyrn7id5p0f7r74mj6qx6ngjfd6cm2vn1s"))

(define rust-yoke-0.8.3
  (crate-source "yoke" "0.8.3"
                "1xgyj6c2lxj2bp891ynmhws87c6z7yyv2li1v0ss9di40hxf57vh"))

(define rust-yoke-derive-0.8.3
  (crate-source "yoke-derive" "0.8.3"
                "0y1a857vmqk2zpq4jj4sxxm7mla18xsrapjldpmvq3g4pql1909k"))

(define rust-zerofrom-0.1.8
  (crate-source "zerofrom" "0.1.8"
                "0wjjdj7gdmd0iq91gzkxl7dlv0nhkk80l4bmdpzh3a1yh48mmh0f"))

(define rust-zerofrom-derive-0.1.8
  (crate-source "zerofrom-derive" "0.1.8"
                "0jhdy4p9c6ja3dynsdck14g2iilq8ai5ch6nsi455x67ys1lcnzp"))

(define rust-zerotrie-0.2.5
  (crate-source "zerotrie" "0.2.5"
                "0gss16krjzk22m57dz5hkdjg99ibj6pa41qr68na7w1jpp1nk8jf"))

(define rust-zerovec-0.11.8
  (crate-source "zerovec" "0.11.8"
                "1n3xlvyba8riys9s8awy4xp533phqycr78nbsmvdkh86g3hn815v"))

(define rust-zerovec-derive-0.11.6
  (crate-source "zerovec-derive" "0.11.6"
                "1ni5j8v99x3fcf3l8kp64b7aq4vf8y22jshfq74xs9mxkp1nzprl"))

(define rust-zmij-1.0.23
  (crate-source "zmij" "1.0.23"
                "06zwri21nnrl34rwinmvbciap8yk1mrl8qfg9pff7lgspc56sri9"))


(define-cargo-inputs lookup-cargo-inputs
  (guile-lsp =>
   (list
         rust-async-trait-0.1.92
         rust-auto-impl-1.3.0
         rust-bitflags-1.3.2
         rust-bitflags-2.13.2
         rust-bytes-1.12.1
         rust-cfg-if-1.0.5
         rust-dashmap-5.5.3
         rust-displaydoc-0.2.7
         rust-errno-0.3.14
         rust-form-urlencoded-1.2.2
         rust-futures-0.3.34
         rust-futures-channel-0.3.34
         rust-futures-core-0.3.34
         rust-futures-io-0.3.34
         rust-futures-macro-0.3.34
         rust-futures-sink-0.3.34
         rust-futures-task-0.3.34
         rust-futures-util-0.3.34
         rust-hashbrown-0.14.5
         rust-httparse-1.10.1
         rust-icu-collections-2.3.0
         rust-icu-locale-core-2.3.0
         rust-icu-normalizer-2.3.0
         rust-icu-normalizer-data-2.3.0
         rust-icu-properties-2.3.0
         rust-icu-properties-data-2.3.0
         rust-icu-provider-2.3.1
         rust-idna-1.1.0
         rust-idna-adapter-1.2.2
         rust-itoa-1.0.18
         rust-libc-0.2.189
         rust-litemap-0.8.3
         rust-lock-api-0.4.14
         rust-lsp-types-0.94.1
         rust-memchr-2.8.3
         rust-mio-1.2.3
         rust-once-cell-1.21.4
         rust-parking-lot-core-0.9.12
         rust-percent-encoding-2.3.2
         rust-pin-project-1.1.13
         rust-pin-project-internal-1.1.13
         rust-pin-project-lite-0.2.17
         rust-potential-utf-0.1.6
         rust-proc-macro2-1.0.107
         rust-quote-1.0.47
         rust-redox-syscall-0.5.18
         rust-scopeguard-1.2.0
         rust-serde-1.0.229
         rust-serde-core-1.0.229
         rust-serde-derive-1.0.229
         rust-serde-json-1.0.151
         rust-serde-repr-0.1.21
         rust-signal-hook-registry-1.4.8
         rust-slab-0.4.12
         rust-smallvec-1.16.1
         rust-stable-deref-trait-1.2.1
         rust-syn-2.0.119
         rust-syn-3.0.6
         rust-synstructure-0.14.0
         rust-tinystr-0.8.4
         rust-tokio-1.53.1
         rust-tokio-macros-2.7.2
         rust-tokio-util-0.7.19
         rust-tower-0.4.13
         rust-tower-layer-0.3.3
         rust-tower-lsp-0.20.0
         rust-tower-lsp-macros-0.9.0
         rust-tower-service-0.3.3
         rust-tracing-0.1.44
         rust-tracing-attributes-0.1.31
         rust-tracing-core-0.1.36
         rust-unicode-ident-1.0.26
         rust-url-2.5.8
         rust-utf8-iter-1.0.4
         rust-wasi-0.11.1+wasi-snapshot-preview1
         rust-windows-link-0.2.1
         rust-windows-sys-0.61.2
         rust-writeable-0.6.4
         rust-yoke-0.8.3
         rust-yoke-derive-0.8.3
         rust-zerofrom-0.1.8
         rust-zerofrom-derive-0.1.8
         rust-zerotrie-0.2.5
         rust-zerovec-0.11.8
         rust-zerovec-derive-0.11.6
         rust-zmij-1.0.23
   )))
