package demo

import scala.collection.mutable

trait Labelled {
  def label: String
}

enum State {
  case Ready, Failed
}

case class User(name: String) extends Labelled {
  def label: String = name
}

object Helpers {
  def normalize(value: String): String = value.trim
  val version = 1
}

type Mapper = String => String
