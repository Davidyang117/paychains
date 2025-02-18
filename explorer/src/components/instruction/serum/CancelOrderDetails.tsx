import React from "react";
import { SignatureResult, TransactionInstruction } from "@paychains/web3.js";
import { InstructionCard } from "../InstructionCard";
import { Address } from "components/common/Address";
import { CancelOrder } from "./types";

export function CancelOrderDetailsCard(props: {
  ix: TransactionInstruction;
  index: number;
  result: SignatureResult;
  info: CancelOrder;
  innerCards?: JSX.Element[];
  childIndex?: number;
}) {
  const { ix, index, result, info, innerCards, childIndex } = props;

  return (
    <InstructionCard
      ix={ix}
      index={index}
      result={result}
      title="Serum Program: Cancel Order"
      innerCards={innerCards}
      childIndex={childIndex}
    >
      <tr>
        <td>Program</td>
        <td className="text-lg-end">
          <Address pubkey={info.programId} alignRight link />
        </td>
      </tr>

      <tr>
        <td>Market</td>
        <td className="text-lg-end">
          <Address pubkey={info.accounts.market} alignRight link />
        </td>
      </tr>

      <tr>
        <td>Open Orders</td>
        <td className="text-lg-end">
          <Address pubkey={info.accounts.openOrders} alignRight link />
        </td>
      </tr>

      <tr>
        <td>Open Orders Owner</td>
        <td className="text-lg-end">
          <Address pubkey={info.accounts.openOrdersOwner} alignRight link />
        </td>
      </tr>

      <tr>
        <td>Request Queue</td>
        <td className="text-lg-end">
          <Address pubkey={info.accounts.requestQueue} alignRight link />
        </td>
      </tr>

      <tr>
        <td>Side</td>
        <td className="text-lg-end">{info.data.side}</td>
      </tr>

      <tr>
        <td>Open Orders Slot</td>
        <td className="text-lg-end">{info.data.openOrdersSlot}</td>
      </tr>

      <tr>
        <td>Order Id</td>
        <td className="text-lg-end">{info.data.orderId.toString(10)}</td>
      </tr>
    </InstructionCard>
  );
}
