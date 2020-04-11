/// Implementation of https://arxiv.org/pdf/1812.10844.pdf
///
/// Deviations from AT2 as defined in the paper
/// 1.  DONE: we decompose dependency tracking from the distributed algorithm
/// 3.  TODO: we genaralize over the distributed algorithm
/// 4.  TODO: seperate out resources from identity (a process id both identified an agent and an account) we generalize this so that
use std::collections::{BTreeSet, HashMap, HashSet};
use std::mem;

use crdts::{CmRDT, Dot, VClock};

use crate::at2::bank::{Account, Bank, Identity, Money, Transfer};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Msg {
    op: Transfer,
    source_version: Dot<Identity>,
}

#[derive(Debug)]
struct Proc {
    // The name this process goes by
    id: Identity,

    // The global bank we are keeping in sync across all procs in the network
    bank: Bank,

    // Applied versions
    seq: VClock<Identity>,

    // Received but not necessarily applied versions
    rec: VClock<Identity>,

    // Set of delivered (but not validated) transfers
    to_validate: Vec<(Identity, Msg)>,

    // Operations that are causally related to the next operation on a given account
    peers: HashSet<Identity>,
}

impl Proc {
    fn new(id: Identity, initial_balance: Money) -> Self {
        let mut proc = Proc {
            id,
            bank: Bank::new(id),
            seq: VClock::new(),
            rec: VClock::new(),
            to_validate: Vec::new(),
            peers: HashSet::new(),
        };

        proc.bank.onboard_account(id, initial_balance);

        proc
    }

    fn onboard(&self) -> Vec<Cmd> {
        vec![Cmd::BroadcastNewPeer {
            new_peer: self.id,
            initial_balance: self.bank.initial_balance(self.id),
        }]
    }

    fn transfer(&self, from: Identity, to: Identity, amount: Money) -> Vec<Cmd> {
        assert_eq!(from, self.id);
        match self.bank.transfer(from, to, amount) {
            Some(transfer) => vec![Cmd::BroadcastMsg {
                from: from,
                msg: Msg {
                    op: transfer,
                    source_version: self.seq.inc(from),
                },
            }],
            None => vec![],
        }
    }

    fn read(&self, account: Account) -> Money {
        self.bank.read(account)
    }

    /// Executed when we successfully deliver messages to process p
    fn on_delivery(&mut self, from: Identity, msg: Msg) {
        assert_eq!(from, msg.source_version.actor);

        // Secure broadcast callback
        if msg.source_version == self.rec.inc(from) {
            println!(
                "{} Accepted message from {} and enqueued for validation",
                self.id, from
            );
            self.rec.apply(msg.source_version);
            self.to_validate.push((from, msg));
        } else {
            println!(
                "{} Rejected message from {}, transfer source version is invalid: {:?}",
                self.id, from, msg.source_version
            );
        }
    }

    /// Executed when a transfer from `from` becomes valid.
    fn on_validated(&mut self, from: Identity, msg: Msg) {
        assert!(self.valid(from, &msg));
        assert_eq!(msg.source_version, self.seq.inc(from));

        // TODO: rename Proc::seq to Proc::knowledge ala. VVwE
        // TODO: rename Proc::rec to Proc::forward_knowledge ala. VVwE
        // TODO: add test that "forward_knowleged >= knowledge" is invariant
        self.seq.apply(msg.source_version);

        // Finally, apply the operation to the underlying algorithm
        self.bank.apply(msg.op);
    }

    fn validate(&self, from: Identity, msg: &Msg) -> bool {
        if from != msg.source_version.actor {
            println!(
                "[INVALID] Transfer from {:?} does not match the msg source version {:?}",
                from, msg.source_version
            );
            false
        } else if msg.source_version != self.seq.inc(from) {
            println!(
                "[INVALID] Source version {:?} is not a direct successor of last transfer from {}: {:?}",
                msg.source_version, from, self.seq.dot(from)
            );
            false
        } else {
            // Finally, check with the underlying algorithm
            self.bank.validate(from, &msg.op)
        }
    }

    fn handle_new_peer(&mut self, new_proc: Identity, initial_balance: Money) -> Vec<Cmd> {
        if !self.peers.contains(&new_proc) {
            // this is a new peer
            self.peers.insert(new_proc);
            self.bank.onboard_account(new_proc, initial_balance);

            // broadcast this proc so that the new peer will discover initial balances
            // TODO: broadcast here is a bit overkill, just need a direct 1-1
            //       communication with the new proc.
            vec![Cmd::BroadcastNewPeer {
                new_peer: self.id,
                initial_balance: self.bank.initial_balance(self.id),
            }]
        } else {
            // We already have this peer, do nothing
            vec![]
        }
    }

    fn handle_msg(&mut self, from: Identity, msg: Msg) -> Vec<Cmd> {
        self.on_delivery(from, msg);
        self.process_msg_queue();
        vec![]
    }

    fn process_msg_queue(&mut self) {
        let to_validate = mem::replace(&mut self.to_validate, Vec::new());
        for (to, msg) in to_validate {
            if self.valid(to, &msg) {
                self.on_validated(to, msg);
            } else {
                println!("[DROP] invalid message detected {:?}", (to, msg));
            }
        }
    }
}
