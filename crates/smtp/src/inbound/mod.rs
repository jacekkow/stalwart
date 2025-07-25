/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::borrow::Cow;

use common::config::smtp::auth::{ArcSealer, DkimSigner};
use mail_auth::{
    ArcOutput, AuthenticatedMessage, AuthenticationResults, DkimResult, DmarcResult, IprevResult,
    SpfResult, arc::ArcSet, dkim::Signature, dmarc::Policy,
};

pub mod auth;
pub mod data;
pub mod ehlo;
pub mod hooks;
pub mod mail;
pub mod milter;
pub mod rcpt;
pub mod session;
pub mod spam;
pub mod spawn;
pub mod vrfy;

#[derive(Debug, Default)]
pub struct FilterResponse {
    pub message: Cow<'static, str>,
    pub disconnect: bool,
}

pub trait ArcSeal {
    fn seal<'x>(
        &self,
        message: &'x AuthenticatedMessage,
        results: &'x AuthenticationResults,
        arc_output: &'x ArcOutput,
    ) -> mail_auth::Result<ArcSet<'x>>;
}

impl ArcSeal for ArcSealer {
    fn seal<'x>(
        &self,
        message: &'x AuthenticatedMessage,
        results: &'x AuthenticationResults,
        arc_output: &'x ArcOutput,
    ) -> mail_auth::Result<ArcSet<'x>> {
        match self {
            ArcSealer::RsaSha256(sealer) => sealer.seal(message, results, arc_output),
            ArcSealer::Ed25519Sha256(sealer) => sealer.seal(message, results, arc_output),
        }
    }
}

pub trait DkimSign {
    fn sign(&self, message: &[u8]) -> mail_auth::Result<Signature>;
    fn sign_chained(&self, message: &[&[u8]]) -> mail_auth::Result<Signature>;
}

impl DkimSign for DkimSigner {
    fn sign(&self, message: &[u8]) -> mail_auth::Result<Signature> {
        match self {
            DkimSigner::RsaSha256(signer) => signer.sign(message),
            DkimSigner::Ed25519Sha256(signer) => signer.sign(message),
        }
    }
    fn sign_chained(&self, message: &[&[u8]]) -> mail_auth::Result<Signature> {
        match self {
            DkimSigner::RsaSha256(signer) => signer.sign_chained(message.iter().copied()),
            DkimSigner::Ed25519Sha256(signer) => signer.sign_chained(message.iter().copied()),
        }
    }
}

pub trait AuthResult {
    fn as_str(&self) -> &'static str;
}

impl AuthResult for SpfResult {
    fn as_str(&self) -> &'static str {
        match self {
            SpfResult::Pass => "pass",
            SpfResult::Fail => "fail",
            SpfResult::SoftFail => "softfail",
            SpfResult::Neutral => "neutral",
            SpfResult::None => "none",
            SpfResult::TempError => "temperror",
            SpfResult::PermError => "permerror",
        }
    }
}

impl AuthResult for IprevResult {
    fn as_str(&self) -> &'static str {
        match self {
            IprevResult::Pass => "pass",
            IprevResult::Fail(_) => "fail",
            IprevResult::TempError(_) => "temperror",
            IprevResult::PermError(_) => "permerror",
            IprevResult::None => "none",
        }
    }
}

impl AuthResult for DkimResult {
    fn as_str(&self) -> &'static str {
        match self {
            DkimResult::Pass => "pass",
            DkimResult::None => "none",
            DkimResult::Neutral(_) => "neutral",
            DkimResult::Fail(_) => "fail",
            DkimResult::PermError(_) => "permerror",
            DkimResult::TempError(_) => "temperror",
        }
    }
}

impl AuthResult for DmarcResult {
    fn as_str(&self) -> &'static str {
        match self {
            DmarcResult::Pass => "pass",
            DmarcResult::Fail(_) => "fail",
            DmarcResult::TempError(_) => "temperror",
            DmarcResult::PermError(_) => "permerror",
            DmarcResult::None => "none",
        }
    }
}

impl AuthResult for Policy {
    fn as_str(&self) -> &'static str {
        match self {
            Policy::Reject => "reject",
            Policy::Quarantine => "quarantine",
            Policy::None | Policy::Unspecified => "none",
        }
    }
}

impl FilterResponse {
    pub fn accept() -> Self {
        Self {
            message: Cow::Borrowed("250 2.0.0 Message queued for delivery.\r\n"),
            disconnect: false,
        }
    }

    pub fn reject() -> Self {
        Self {
            message: Cow::Borrowed("503 5.5.3 Message rejected.\r\n"),
            disconnect: false,
        }
    }

    pub fn temp_fail() -> Self {
        Self {
            message: Cow::Borrowed("451 4.3.5 Unable to accept message at this time.\r\n"),
            disconnect: false,
        }
    }

    pub fn shutdown() -> Self {
        Self {
            message: Cow::Borrowed("421 4.3.0 Server shutting down.\r\n"),
            disconnect: true,
        }
    }

    pub fn server_failure() -> Self {
        Self {
            message: Cow::Borrowed("451 4.3.5 Unable to accept message at this time.\r\n"),
            disconnect: false,
        }
    }

    pub fn disconnect(self) -> Self {
        Self {
            disconnect: true,
            ..self
        }
    }

    pub fn into_bytes(self) -> Cow<'static, [u8]> {
        match self.message {
            Cow::Borrowed(s) => Cow::Borrowed(s.as_bytes()),
            Cow::Owned(s) => Cow::Owned(s.into_bytes()),
        }
    }
}
