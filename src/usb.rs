use bincode::config::Options;
use rusb::{Context, DeviceHandle, Error, Result};
use serde::{Deserialize, Serialize};
use std::mem;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum Direction {
    In,
    Out,
}

#[repr(C)]
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct CommandBlockWrapper {
    signature: [u8; 4],
    tag: u32,
    data_transfer_length: u32,
    flags: u8,
    logical_unit_number: u8,
    command_length: u8,
    command_data: [u8; 16],
}

#[repr(C)]
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct CommandStatusWrapper {
    signature: [u8; 4],
    tag: u32,
    data_residue: u32,
    status: u8,
}

static TAG: AtomicU32 = AtomicU32::new(1);

pub struct ScsiOverUsbConnection {
    pub device_handle: DeviceHandle<Context>,
    pub endpoint_out: u8,
    pub endpoint_in: u8,
    pub timeout: Duration,
}

impl ScsiOverUsbConnection {
    pub fn read_command<T: serde::de::DeserializeOwned, O: bincode::config::Options>(
        &mut self,
        command: &[u8; 16],
        bincode_options: O,
    ) -> Result<T> {
        let length = mem::size_of::<T>();
        let cbw_data = &get_command_block_wrapper(command, length as u32, Direction::In);
        self.device_handle
            .write_bulk(self.endpoint_out, cbw_data, self.timeout)?;
        let mut buf: Vec<u8> = vec![0; length];
        self.device_handle
            .read_bulk(self.endpoint_in, &mut buf, self.timeout)?;
        self.send_status_block_wrapper()?;
        let result: T = bincode_options
            .with_fixint_encoding()
            .deserialize(&buf)
            .unwrap();
        Ok(result)
    }

    pub fn write_command<T: Serialize, O: bincode::config::Options>(
        &mut self,
        command: &[u8; 16],
        value: T,
        data: &[u8],
        bincode_options: O,
    ) -> Result<()> {
        let mut value_data: Vec<u8> = bincode_options
            .with_fixint_encoding()
            .serialize(&value)
            .unwrap();
        let mut bulk_data: Vec<u8> = Vec::new();
        bulk_data.append(&mut value_data);
        bulk_data.extend_from_slice(data);
        let cbw_data =
            &get_command_block_wrapper(command, bulk_data.len() as u32, Direction::Out);
        self.device_handle
            .write_bulk(self.endpoint_out, cbw_data, self.timeout)?;
        self.device_handle
            .write_bulk(self.endpoint_out, &bulk_data, self.timeout)?;
        self.send_status_block_wrapper()?;
        Ok(())
    }

    pub fn write_command_no_data(&mut self, command: &[u8; 16]) -> Result<usize> {
        let cbw_data = &get_command_block_wrapper(command, 0, Direction::Out);
        let data_written = self
            .device_handle
            .write_bulk(self.endpoint_out, cbw_data, self.timeout)?;
        self.send_status_block_wrapper()?;
        Ok(data_written)
    }

    fn send_status_block_wrapper(&mut self) -> Result<CommandStatusWrapper> {
        let mut csb_data: [u8; 13] = [0; 13];
        loop {
            match self
                .device_handle
                .read_bulk(self.endpoint_in, &mut csb_data, self.timeout)
            {
                Ok(_) => {
                    return Ok(bincode::options()
                        .with_fixint_encoding()
                        .deserialize::<CommandStatusWrapper>(&csb_data)
                        .unwrap());
                }
                Err(error) => match error {
                    Error::Pipe => {
                        self.device_handle.clear_halt(self.endpoint_in).unwrap();
                        continue;
                    }
                    _ => return Err(error),
                },
            }
        }
    }
}

pub fn get_command_block_wrapper(
    command_data: &[u8; 16],
    data_transfer_length: u32,
    direction: Direction,
) -> Vec<u8> {
    let flags: u8 = match direction {
        Direction::In => 0x80,
        Direction::Out => 0x00,
    };
    let tag = TAG.fetch_add(1, Ordering::SeqCst);
    let cwb = CommandBlockWrapper {
        signature: [0x55, 0x53, 0x42, 0x43],
        tag,
        data_transfer_length,
        flags,
        logical_unit_number: 0,
        command_length: 16,
        command_data: *command_data,
    };
    bincode::options()
        .with_little_endian()
        .with_fixint_encoding()
        .serialize(&cwb)
        .unwrap()
}
