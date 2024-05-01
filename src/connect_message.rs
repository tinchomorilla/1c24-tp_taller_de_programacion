#[derive(Debug)]
pub struct FixedHeader {
    message_type: u8, // byte 1
    remaining_length: u8, // byte 2
}

#[derive(Debug)]
pub struct VariableHeader {
    protocol_name: [u8; 4], // bytes 1-4
    protocol_level: u8, // byte 6
    connect_flags: ConnectFlags, // byte 7
}


#[derive(Debug)]
#[allow(dead_code)]
pub struct ConnectFlags {
    username_flag: bool, // bit 7
    password_flag: bool, // bit 6
    will_retain: bool, // bit 5
    will_qos: u8, // bits 3-4
    will_flag: bool, // bit 2
    clean_session: bool, // bit 1
    reserved: bool, // bit 0
}

#[derive(Debug)]
pub struct Payload<'a> {
    client_id: &'a str,
    will_topic: Option<&'a str>,
    will_message: Option<&'a str>,
    username: Option<&'a str>,
    password: Option<&'a str>,
}

#[derive(Debug)]
pub struct ConnectMessage<'a> {
    fixed_header: FixedHeader,
    variable_header: VariableHeader,
    payload: Payload<'a>,
}

impl <'a> ConnectMessage<'a> {
    pub fn new(
        message_type: u8,
        client_id: &'a str,
        will_topic: Option<&'a str>,
        will_message: Option<&'a str>,
        username: Option<&'a str>,
        password: Option<&'a str>,
    ) -> Self {
        let fixed_header = FixedHeader {
            message_type,
            remaining_length: 0, // Se actualizará más tarde
        };

        let variable_header = VariableHeader {
            protocol_name: [77, 81, 84, 84], // "MQTT" en ASCII
            protocol_level: 4, // MQTT 3.1.1
            connect_flags: ConnectFlags {
                username_flag: username.is_some(),
                password_flag: password.is_some(),
                will_retain: false,
                will_qos: 0,
                will_flag: will_topic.is_some() && will_message.is_some(),
                clean_session: true,
                reserved: false,
            },
        };

        let payload = Payload {
            client_id,
            will_topic,
            will_message,
            username,
            password,
        };

        let mut connect_message = ConnectMessage {
            fixed_header,
            variable_header,
            payload,
        };

        connect_message.fixed_header.remaining_length = connect_message.calculate_remaining_length();

        connect_message
    }
    fn calculate_remaining_length(&self) -> u8 {
        let variable_header_length = 5 + 1 + 1; // 5 bytes for "MQTT", 1 byte for level, 1 byte for connect flags
        let payload_length = self.payload.client_id.len()
            + self.payload.will_topic.map_or(0, |s| s.len())
            + self.payload.will_message.map_or(0, |s| s.len())
            + self.payload.username.map_or(0, |s| s.len())
            + self.payload.password.map_or(0, |s| s.len());

        (variable_header_length + payload_length) as u8
    }

    pub fn to_bytes(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Fixed Header
        bytes.push(self.fixed_header.message_type);
        self.fixed_header.remaining_length = self.calculate_remaining_length();
        bytes.push(self.fixed_header.remaining_length);

        // Variable Header
        bytes.extend_from_slice(&self.variable_header.protocol_name);
        bytes.push(self.variable_header.protocol_level);
        let connect_flags = self.variable_header.connect_flags.to_byte();
        bytes.push(connect_flags);

        // Payload
        bytes.extend_from_slice(self.payload.client_id.as_bytes());
        if let Some(will_topic) = self.payload.will_topic {
            bytes.extend_from_slice(will_topic.as_bytes());
        }
        if let Some(will_message) = self.payload.will_message {
            bytes.extend_from_slice(will_message.as_bytes());
        }
        if let Some(username) = self.payload.username {
            bytes.extend_from_slice(username.as_bytes());
        }
        if let Some(password) = self.payload.password {
            bytes.extend_from_slice(password.as_bytes());
        }

        bytes
    }
}

impl ConnectFlags {
    fn to_byte(&self) -> u8 {
        let mut byte = 0;
        if self.username_flag {
            byte |= 0x80;
        }
        if self.password_flag {
            byte |= 0x40;
        }
        if self.will_retain {
            byte |= 0x20;
        }
        byte |= (self.will_qos & 0x03) << 3;
        if self.will_flag {
            byte |= 0x04;
        }
        if self.clean_session {
            byte |= 0x02;
        }
        byte
    }
}