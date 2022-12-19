// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::connection::Connection;
use crate::connection::HmacSha256;
use anyhow::anyhow;
use anyhow::bail;
use anyhow::Result;
use bytes::Bytes;
use chrono::Utc;
use generic_array::GenericArray;
use json::JsonValue;
use json::{self};
use std::fmt;
use std::{self};
use uuid::Uuid;

struct RawMessage {
    zmq_identities: Vec<Bytes>,
    jparts: Vec<Bytes>,
}

impl RawMessage {
    pub(crate) async fn read<S: zeromq::SocketRecv>(
        connection: &mut Connection<S>,
    ) -> Result<RawMessage> {
        Self::from_multipart(connection.socket.recv().await?, connection)
    }

    pub(crate) fn from_multipart<S>(
        multipart: zeromq::ZmqMessage,
        connection: &Connection<S>,
    ) -> Result<RawMessage> {
        let delimiter_index = multipart
            .iter()
            .position(|part| &part[..] == DELIMITER)
            .ok_or_else(|| anyhow!("Missing delimeter"))?;
        let mut parts = multipart.into_vec();
        let jparts: Vec<_> = parts.drain(delimiter_index + 2..).collect();
        let hmac = parts.pop().unwrap();
        // Remove delimiter, so that what's left is just the identities.
        parts.pop();
        let zmq_identities = parts;

        let raw_message = RawMessage {
            zmq_identities,
            jparts,
        };

        if let Some(mac_template) = &connection.mac {
            let mut mac = mac_template.clone();
            raw_message.digest(&mut mac);
            use hmac::Mac;
            if let Err(error) = mac.verify(GenericArray::from_slice(&hex::decode(&hmac)?)) {
                bail!("{}", error);
            }
        }

        Ok(raw_message)
    }

    async fn send<S: zeromq::SocketSend>(self, connection: &mut Connection<S>) -> Result<()> {
        use hmac::Mac;
        let hmac = if let Some(mac_template) = &connection.mac {
            let mut mac = mac_template.clone();
            self.digest(&mut mac);
            hex::encode(mac.finalize().into_bytes().as_slice())
        } else {
            String::new()
        };
        let mut parts: Vec<bytes::Bytes> = Vec::new();
        for part in &self.zmq_identities {
            parts.push(part.to_vec().into());
        }
        parts.push(DELIMITER.into());
        parts.push(hmac.as_bytes().to_vec().into());
        for part in &self.jparts {
            parts.push(part.to_vec().into());
        }
        // ZmqMessage::try_from only fails if parts is empty, which it never
        // will be here.
        let message = zeromq::ZmqMessage::try_from(parts).unwrap();
        connection.socket.send(message).await?;
        Ok(())
    }

    fn digest(&self, mac: &mut HmacSha256) {
        use hmac::Mac;
        for part in &self.jparts {
            mac.update(part);
        }
    }
}

#[derive(Clone)]
pub(crate) struct JupyterMessage {
    zmq_identities: Vec<Bytes>,
    header: JsonValue,
    parent_header: JsonValue,
    metadata: JsonValue,
    content: JsonValue,
}

const DELIMITER: &[u8] = b"<IDS|MSG>";

impl JupyterMessage {
   