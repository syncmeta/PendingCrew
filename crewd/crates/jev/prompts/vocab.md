<!--
代码算好的量换成的词（jaggedness #2/#3：时间、计数、排名不给 Jev 原始数字）。
分档边界在 src/decisions/mod.rs（recency_word / wait_word / mentions_word），改档位要同时改那里的测试。
-->

## status.busy
正在忙

## status.idle
空闲

## recency.recent
刚刚还在活跃

## recency.middle
不久前活跃过

## recency.stale
很久没有活跃

## kind.message
群消息

## kind.todo
待办

## kind.work_item
工作项

## kind.channel
群

## wait.minutes
刚问不久

## wait.hours
等了几个小时

## wait.day
等了大半天到一天

## wait.days
等了好几天

## mentions.none
没人叫人类

## mentions.some
有人叫过人类

## mentions.many
好几次在叫人类
